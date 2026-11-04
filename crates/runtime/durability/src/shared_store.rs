use std::future::Future;

use nanocode_platform::Instant;
use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;

use crate::{
    Committed, EventHits, EventPage, EventQuery, EventRange, KnownSummary, ListedState,
    OwnedCommit, OwnedState, OwnerId, OwnerToken, StateCommit, StatePage, StateQuery, StateReader,
    StateStore, StoreError, StoreEvent, StoreFuture, StoredState, SummaryUpdate,
};
#[cfg(not(target_family = "wasm"))]
use crate::{Error, StoreUpdate};

const COMMAND_CAPACITY: usize = 64;

enum Command {
    ReadRecords {
        state_id: String,
        keys: Vec<String>,
        result: oneshot::Sender<Result<Vec<Option<String>>, StoreError>>,
    },
    ReadRecord {
        state_id: String,
        key: String,
        result: oneshot::Sender<Result<Option<String>, StoreError>>,
    },
    Acquire {
        state_id: String,
        owner_id: OwnerId,
        result: oneshot::Sender<Result<OwnedState, StoreError>>,
    },
    #[cfg(not(target_family = "wasm"))]
    AcquireWith {
        state_id: String,
        owner_id: OwnerId,
        update: StoreUpdate,
        result: oneshot::Sender<Result<OwnedState, StoreError>>,
    },
    #[cfg(not(target_family = "wasm"))]
    AcquireWithCompanion {
        state_id: String,
        owner_id: OwnerId,
        update: StoreUpdate,
        companion_id: String,
        companion_owner: OwnerId,
        result: oneshot::Sender<Result<(OwnedState, OwnedState), StoreError>>,
    },
    Replace {
        state_id: String,
        owner: OwnerToken,
        expected_revision: u64,
        payload: String,
        records: Vec<crate::StoreRecord>,
        result: oneshot::Sender<Result<u64, StoreError>>,
    },
    ReplaceLatest {
        state_id: String,
        owner: OwnerToken,
        expected_revision: u64,
        write: std::sync::Arc<LatestWrite>,
        result: oneshot::Sender<Result<u64, StoreError>>,
    },
    ReplaceDeferred {
        state_id: String,
        owner: OwnerToken,
        expected_revision: u64,
        content: DeferredContent,
        result: oneshot::Sender<Result<u64, StoreError>>,
    },
    Commit {
        state_id: String,
        owner: OwnerToken,
        expected_revision: u64,
        payload: String,
        records: Vec<crate::StoreRecord>,
        events: Vec<StoreEvent>,
        summary: SummaryUpdate,
        result: oneshot::Sender<Result<Committed, StoreError>>,
    },
    Peek {
        state_id: String,
        result: oneshot::Sender<Result<crate::StoredState, StoreError>>,
    },
    Owner {
        state_id: String,
        result: oneshot::Sender<Result<Option<OwnerToken>, StoreError>>,
    },
    ListStates {
        limit: usize,
        result: oneshot::Sender<Result<Vec<String>, StoreError>>,
    },
    ListRecords {
        state_id: String,
        result: oneshot::Sender<Result<Vec<String>, StoreError>>,
    },
    ListSummaries {
        limit: usize,
        result: oneshot::Sender<Result<Option<Vec<ListedState>>, StoreError>>,
    },
    RememberSummaries {
        summaries: Vec<KnownSummary>,
        result: oneshot::Sender<Result<(), StoreError>>,
    },
    Flush {
        result: oneshot::Sender<Result<(), StoreError>>,
    },
}

#[derive(Clone)]
enum Latest {
    Open {
        payload: std::sync::Arc<str>,
        records: imbl::Vector<crate::StoreRecord>,
        keys: imbl::HashSet<String>,
    },
    Taken,
}

pub(crate) struct LatestWrite(nanocode_sync::Atom<Latest>);

impl LatestWrite {
    pub(crate) fn update(&self, payload: &str, records: &[crate::StoreRecord]) -> bool {
        let payload: std::sync::Arc<str> = std::sync::Arc::from(payload);
        self.0
            .modify(|latest| match latest {
                Latest::Open {
                    payload: current,
                    records: staged,
                    keys,
                } => {
                    *current = std::sync::Arc::clone(&payload);
                    for record in records {
                        if keys.insert(record.key.clone()).is_none() {
                            staged.push_back(record.clone());
                        }
                    }
                    Ok(())
                }
                Latest::Taken => Err(()),
            })
            .is_ok()
    }

    fn take(&self) -> (String, Vec<crate::StoreRecord>) {
        match self
            .0
            .update(|latest| std::mem::replace(latest, Latest::Taken))
        {
            Latest::Open {
                payload, records, ..
            } => (payload.to_string(), records.into_iter().collect()),
            Latest::Taken => (String::new(), Vec::new()),
        }
    }
}

struct StoreRequest {
    command: Command,
    span: tracing::Span,
    queued: Instant,
}

#[derive(Clone)]
pub struct SharedStore {
    commands: mpsc::UnboundedSender<StoreRequest>,
    batches_companion_acquisition: bool,
    supports_events: bool,
    supports_summaries: bool,
    pub(crate) terminal_receipt_limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct WeakSharedStore(mpsc::WeakUnboundedSender<StoreRequest>);

impl WeakSharedStore {
    pub(crate) fn is(&self, store: &SharedStore) -> bool {
        self.0
            .upgrade()
            .is_some_and(|commands| commands.same_channel(&store.commands))
    }
}

impl SharedStore {
    pub(crate) fn same_store(&self, other: &Self) -> bool {
        self.commands.same_channel(&other.commands)
    }

    pub(crate) fn downgrade(&self) -> WeakSharedStore {
        WeakSharedStore(self.commands.downgrade())
    }

    pub(crate) fn new<S>(store: S) -> crate::Result<Self>
    where
        S: StateStore + 'static,
    {
        #[cfg(not(target_family = "wasm"))]
        let batches_companion_acquisition = store.batches_companion_acquisition();
        #[cfg(target_family = "wasm")]
        let batches_companion_acquisition = false;
        let supports_events = store.supports_events();
        let supports_summaries = store.supports_summaries();
        let (commands, receiver) = mpsc::unbounded_channel();
        if store.concurrent_handle().is_some() {
            spawn_driver(drive_concurrently(store, receiver))?;
        } else {
            spawn_driver(drive_grouped(store, receiver))?;
        }
        Ok(Self {
            commands,
            batches_companion_acquisition,
            supports_events,
            supports_summaries,
            terminal_receipt_limit: None,
        })
    }
}

async fn drive_grouped<S: StateStore>(
    mut store: S,
    mut receiver: mpsc::UnboundedReceiver<StoreRequest>,
) {
    let (finished, mut completions) = mpsc::unbounded_channel();
    let mut operations = 0_usize;
    let mut groups = 0_usize;
    let mut queued = std::collections::VecDeque::<StoreRequest>::new();
    let mut barriers = 0_usize;
    let mut open = true;
    let writers = store.group_writers().max(1);
    let mut forming = Vec::new();
    let mut states = std::collections::HashSet::new();
    let mut in_flight = std::collections::HashSet::<String>::new();
    let mut blocked = 0_usize;
    loop {
        while queued.front().is_some_and(|request| {
            command_state(&request.command).is_some_and(|state| {
                !states.contains(state) && (groups >= writers || !in_flight.contains(state))
            })
        }) {
            let Some(request) = queued.pop_front() else {
                break;
            };
            match Grouped::from_request(request) {
                Ok(mut grouped) => {
                    if groups > 0 {
                        grouped.prefetch();
                    }
                    if in_flight.contains(grouped.state_id()) {
                        blocked += 1;
                    }
                    states.insert(grouped.state_id().to_owned());
                    forming.push(grouped);
                }
                Err(request) => {
                    queued.push_front(request);
                    break;
                }
            }
        }
        if groups < writers && blocked == 0 && !forming.is_empty() {
            let group = std::mem::take(&mut forming);
            let states = std::mem::take(&mut states);
            match store.pipeline_handle() {
                Some(mut handle) => {
                    groups += 1;
                    in_flight.extend(states.iter().cloned());
                    let completion = Completion(finished.clone(), states.into_iter().collect());
                    spawn_operation(async move {
                        run_grouped(&mut *handle, group).await;
                        drop(completion);
                    });
                }
                None => run_grouped(&mut store, group).await,
            }
            continue;
        }
        if groups == 0
            && forming.is_empty()
            && operations == 0
            && queued
                .front()
                .is_some_and(|request| command_state(&request.command).is_none())
            && let Some(request) = queued.pop_front()
        {
            barriers -= 1;
            run(&mut store, request).await;
            continue;
        }
        if !open && operations == 0 && groups == 0 && queued.is_empty() && forming.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            Some(completed) = completions.recv(), if operations + groups > 0 => {
                if completed.is_empty() {
                    operations -= 1;
                } else {
                    groups -= 1;
                    for state in completed {
                        in_flight.remove(&state);
                        if states.contains(&state) {
                            blocked -= 1;
                        }
                    }
                }
            }
            request = receiver.recv(), if open => match request {
                Some(request) => {
                    let mut next = Some(request);
                    while let Some(request) = next.take() {
                        let write = command_state(&request.command).is_some();
                        let pipelined = !write
                            && barriers == 0
                            && !matches!(request.command, Command::Flush { .. });
                        match pipelined.then(|| store.pipeline_handle()).flatten() {
                            Some(mut handle) => {
                                operations += 1;
                                let completion = Completion(finished.clone(), Vec::new());
                                spawn_operation(async move {
                                    run(&mut *handle, request).await;
                                    drop(completion);
                                });
                            }
                            None => {
                                if !write {
                                    barriers += 1;
                                }
                                queued.push_back(request);
                            }
                        }
                        next = receiver.try_recv().ok();
                    }
                }
                None => open = false,
            },
        }
    }
}

fn command_state(command: &Command) -> Option<&str> {
    match command {
        Command::Replace { state_id, .. }
        | Command::ReplaceLatest { state_id, .. }
        | Command::ReplaceDeferred { state_id, .. }
        | Command::Commit { state_id, .. }
        | Command::Acquire { state_id, .. } => Some(state_id),
        #[cfg(not(target_family = "wasm"))]
        Command::AcquireWith { state_id, .. } => Some(state_id),
        _ => None,
    }
}

async fn run_grouped<S: StateStore + ?Sized>(store: &mut S, mut group: Vec<Grouped>) {
    match group.pop() {
        Some(single) if group.is_empty() => run(store, single.into_request()).await,
        Some(last) => {
            group.push(last);
            run_group(store, group).await;
        }
        None => {}
    }
}

struct Completion(mpsc::UnboundedSender<Vec<String>>, Vec<String>);

impl Drop for Completion {
    fn drop(&mut self) {
        let _ = self.0.send(std::mem::take(&mut self.1));
    }
}

async fn drive_concurrently<S: StateStore>(
    mut store: S,
    mut receiver: mpsc::UnboundedReceiver<StoreRequest>,
) {
    let (finished, mut completions) = mpsc::unbounded_channel();
    let mut in_flight = 0_usize;
    loop {
        let request = tokio::select! {
            biased;
            Some(_) = completions.recv(), if in_flight > 0 => {
                in_flight -= 1;
                continue;
            }
            request = receiver.recv() => match request {
                Some(request) => request,
                None => break,
            },
        };
        let handle = match &request.command {
            Command::Flush { .. } => None,
            _ => store.concurrent_handle(),
        };
        let Some(mut handle) = handle else {
            while in_flight > 0 && completions.recv().await.is_some() {
                in_flight -= 1;
            }
            run(&mut store, request).await;
            continue;
        };
        in_flight += 1;
        let completion = Completion(finished.clone(), Vec::new());
        spawn_operation(async move {
            run(&mut *handle, request).await;
            drop(completion);
        });
    }
}

enum Reply {
    Replace(oneshot::Sender<Result<u64, StoreError>>),
    Commit(oneshot::Sender<Result<Committed, StoreError>>),
}

impl Reply {
    fn send(self, result: Result<Committed, StoreError>) {
        match self {
            Self::Replace(sender) => drop(sender.send(result.map(|committed| committed.revision))),
            Self::Commit(sender) => drop(sender.send(result)),
        }
    }
}

pub(crate) type DeferredContent = std::pin::Pin<
    Box<dyn Future<Output = Result<(String, Vec<crate::StoreRecord>), StoreError>> + Send>,
>;

struct Grouped {
    work: Work,
    span: tracing::Span,
    queued: Instant,
}

enum Work {
    Commit {
        commit: OwnedCommit,
        reply: Reply,
        content: Option<DeferredContent>,
    },
    Acquire {
        state_id: String,
        owner_id: OwnerId,
        #[cfg(not(target_family = "wasm"))]
        update: Option<StoreUpdate>,
        result: oneshot::Sender<Result<OwnedState, StoreError>>,
    },
}

enum Answer {
    Write(Reply),
    #[cfg(not(target_family = "wasm"))]
    Acquired {
        result: oneshot::Sender<Result<OwnedState, StoreError>>,
        owned: OwnedState,
        payload: String,
    },
}

impl Answer {
    fn send(self, result: Result<Committed, StoreError>) {
        match self {
            Self::Write(reply) => reply.send(result),
            #[cfg(not(target_family = "wasm"))]
            Self::Acquired {
                result: sender,
                mut owned,
                payload,
            } => drop(sender.send(match result {
                Ok(committed) => {
                    owned.state = StoredState {
                        revision: committed.revision,
                        payload: Some(payload),
                    };
                    Ok(owned)
                }
                Err(error) => Err(updated_after_acquiring(error)),
            })),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn updated_after_acquiring(error: StoreError) -> StoreError {
    StoreError::Backend(format!(
        "state update failed after acquiring its owner: {error}"
    ))
}

impl Grouped {
    fn prefetch(&mut self) {
        if let Work::Commit { content, .. } = &mut self.work
            && let Some(deferred) = content.take()
        {
            let (sender, receiver) = oneshot::channel();
            spawn_operation(async move {
                drop(sender.send(deferred.await));
            });
            *content = Some(Box::pin(
                async move { receiver.await.map_err(|_| stopped())? },
            ));
        }
    }

    fn state_id(&self) -> &str {
        match &self.work {
            Work::Commit { commit, .. } => &commit.state_id,
            Work::Acquire { state_id, .. } => state_id,
        }
    }

    fn from_request(request: StoreRequest) -> Result<Self, StoreRequest> {
        let StoreRequest {
            command,
            span,
            queued,
        } = request;
        let mut content = None;
        let (commit, reply) = match command {
            Command::Acquire {
                state_id,
                owner_id,
                result,
            } => {
                return Ok(Self {
                    work: Work::Acquire {
                        state_id,
                        owner_id,
                        #[cfg(not(target_family = "wasm"))]
                        update: None,
                        result,
                    },
                    span,
                    queued,
                });
            }
            #[cfg(not(target_family = "wasm"))]
            Command::AcquireWith {
                state_id,
                owner_id,
                update,
                result,
            } => {
                return Ok(Self {
                    work: Work::Acquire {
                        state_id,
                        owner_id,
                        update: Some(update),
                        result,
                    },
                    span,
                    queued,
                });
            }
            Command::ReplaceDeferred {
                state_id,
                owner,
                expected_revision,
                content: deferred,
                result,
            } => {
                content = Some(deferred);
                (
                    OwnedCommit {
                        state_id,
                        owner,
                        expected_revision,
                        payload: String::new(),
                        records: Vec::new(),
                        events: Vec::new(),
                        summary: SummaryUpdate::Unchanged,
                    },
                    Reply::Replace(result),
                )
            }
            Command::Replace {
                state_id,
                owner,
                expected_revision,
                payload,
                records,
                result,
            } => (
                OwnedCommit {
                    state_id,
                    owner,
                    expected_revision,
                    payload,
                    records,
                    events: Vec::new(),
                    summary: SummaryUpdate::Unchanged,
                },
                Reply::Replace(result),
            ),
            Command::ReplaceLatest {
                state_id,
                owner,
                expected_revision,
                write,
                result,
            } => {
                let (payload, records) = write.take();
                (
                    OwnedCommit {
                        state_id,
                        owner,
                        expected_revision,
                        payload,
                        records,
                        events: Vec::new(),
                        summary: SummaryUpdate::Unchanged,
                    },
                    Reply::Replace(result),
                )
            }
            Command::Commit {
                state_id,
                owner,
                expected_revision,
                payload,
                records,
                events,
                summary,
                result,
            } => (
                OwnedCommit {
                    state_id,
                    owner,
                    expected_revision,
                    payload,
                    records,
                    events,
                    summary,
                },
                Reply::Commit(result),
            ),
            command => {
                return Err(StoreRequest {
                    command,
                    span,
                    queued,
                });
            }
        };
        Ok(Self {
            work: Work::Commit {
                commit,
                reply,
                content,
            },
            span,
            queued,
        })
    }

    fn into_request(self) -> StoreRequest {
        let command = match self.work {
            #[cfg(not(target_family = "wasm"))]
            Work::Acquire {
                state_id,
                owner_id,
                update: Some(update),
                result,
            } => Command::AcquireWith {
                state_id,
                owner_id,
                update,
                result,
            },
            Work::Acquire {
                state_id,
                owner_id,
                result,
                ..
            } => Command::Acquire {
                state_id,
                owner_id,
                result,
            },
            Work::Commit {
                commit:
                    OwnedCommit {
                        state_id,
                        owner,
                        expected_revision,
                        payload,
                        records,
                        events,
                        summary,
                    },
                reply,
                content,
            } => match (reply, content) {
                (Reply::Replace(result), Some(content)) => Command::ReplaceDeferred {
                    state_id,
                    owner,
                    expected_revision,
                    content,
                    result,
                },
                (Reply::Replace(result), None) => Command::Replace {
                    state_id,
                    owner,
                    expected_revision,
                    payload,
                    records,
                    result,
                },
                (Reply::Commit(result), _) => Command::Commit {
                    state_id,
                    owner,
                    expected_revision,
                    payload,
                    records,
                    events,
                    summary,
                    result,
                },
            },
        };
        StoreRequest {
            command,
            span: self.span,
            queued: self.queued,
        }
    }
}

struct Acquisition {
    #[cfg(not(target_family = "wasm"))]
    state_id: String,
    #[cfg(not(target_family = "wasm"))]
    update: Option<StoreUpdate>,
    result: oneshot::Sender<Result<OwnedState, StoreError>>,
}

async fn run_group<S: StateStore + ?Sized>(store: &mut S, group: Vec<Grouped>) {
    let mut writes = Vec::with_capacity(group.len());
    let mut acquisitions = Vec::new();
    let mut owners = Vec::new();
    for grouped in group {
        grouped.span.record(
            "queue.duration_ms",
            grouped.queued.elapsed().as_secs_f64() * 1_000.0,
        );
        match grouped.work {
            Work::Commit {
                commit,
                reply,
                content,
            } => writes.push((commit, reply, content)),
            Work::Acquire {
                state_id,
                owner_id,
                #[cfg(not(target_family = "wasm"))]
                update,
                result,
            } => {
                acquisitions.push(Acquisition {
                    #[cfg(not(target_family = "wasm"))]
                    state_id: state_id.clone(),
                    #[cfg(not(target_family = "wasm"))]
                    update,
                    result,
                });
                owners.push((state_id, owner_id));
            }
        }
    }
    let contents = futures_util::future::join_all(
        writes
            .iter_mut()
            .map(|(_, _, content)| content.take())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|content| async move {
                match content {
                    Some(content) => Some(content.await),
                    None => None,
                }
            }),
    );
    let count = owners.len();
    let acquired = async {
        if owners.is_empty() {
            return Vec::new();
        }
        store
            .acquire_many(owners)
            .instrument(tracing::info_span!(
                "durability.store.acquire_group",
                acquire.count = count
            ))
            .await
    };
    let (contents, mut acquired) = futures_util::future::join(contents, acquired).await;
    if acquired.len() != count {
        let error = StoreError::Backend(format!(
            "durability store answered {} of {count} grouped acquisitions",
            acquired.len()
        ));
        acquired = (0..count).map(|_| Err(error.clone())).collect();
    }
    let mut commits = Vec::with_capacity(writes.len());
    let mut answers = Vec::with_capacity(writes.len());
    for ((mut commit, reply, _), content) in writes.into_iter().zip(contents) {
        match content {
            Some(Ok((payload, records))) => {
                commit.payload = payload;
                commit.records = records;
            }
            Some(Err(error)) => {
                reply.send(Err(error));
                continue;
            }
            None => {}
        }
        commits.push(commit);
        answers.push(Answer::Write(reply));
    }
    for (acquisition, owned) in acquisitions.into_iter().zip(acquired) {
        let owned = match owned {
            Ok(owned) => owned,
            Err(error) => {
                drop(acquisition.result.send(Err(error)));
                continue;
            }
        };
        #[cfg(not(target_family = "wasm"))]
        if let Some(update) = acquisition.update {
            match update(&owned.state) {
                Ok(Some(replacement)) => {
                    commits.push(OwnedCommit {
                        state_id: acquisition.state_id,
                        owner: owned.owner.clone(),
                        expected_revision: owned.state.revision,
                        payload: replacement.payload.clone(),
                        records: replacement.records,
                        events: Vec::new(),
                        summary: SummaryUpdate::Unchanged,
                    });
                    answers.push(Answer::Acquired {
                        result: acquisition.result,
                        owned,
                        payload: replacement.payload,
                    });
                }
                Ok(None) => drop(acquisition.result.send(Ok(owned))),
                Err(error) => drop(acquisition.result.send(Err(updated_after_acquiring(error)))),
            }
            continue;
        }
        drop(acquisition.result.send(Ok(owned)));
    }
    let count = commits.len();
    if count == 0 {
        return;
    }
    let results = store
        .commit_many(commits)
        .instrument(tracing::info_span!(
            "durability.store.group",
            commit.count = count
        ))
        .await;
    if results.len() != count {
        let error = StoreError::Backend(format!(
            "durability store answered {} of {count} grouped commits",
            results.len()
        ));
        for answer in answers {
            answer.send(Err(error.clone()));
        }
        return;
    }
    for (answer, result) in answers.into_iter().zip(results) {
        answer.send(result);
    }
}

async fn run<S: StateStore + ?Sized>(store: &mut S, request: StoreRequest) {
    let StoreRequest {
        command,
        span,
        queued,
    } = request;
    span.record(
        "queue.duration_ms",
        queued.elapsed().as_secs_f64() * 1_000.0,
    );
    async {
        match command {
            Command::ReadRecords {
                state_id,
                keys,
                result,
            } => {
                drop(result.send(store.read_records(&state_id, &keys).await));
            }
            Command::ReadRecord {
                state_id,
                key,
                result,
            } => {
                drop(result.send(store.read_record(&state_id, &key).await));
            }

            Command::Acquire {
                state_id,
                owner_id,
                result,
            } => {
                drop(result.send(store.acquire(&state_id, owner_id).await));
            }
            Command::Replace {
                state_id,
                owner,
                expected_revision,
                payload,
                records: new_records,
                result,
            } => {
                drop(
                    result.send(
                        store
                            .replace(&state_id, &owner, expected_revision, &payload, &new_records)
                            .await,
                    ),
                );
            }
            Command::ReplaceLatest {
                state_id,
                owner,
                expected_revision,
                write,
                result,
            } => {
                let (payload, new_records) = write.take();
                drop(
                    result.send(
                        store
                            .replace(&state_id, &owner, expected_revision, &payload, &new_records)
                            .await,
                    ),
                );
            }
            Command::ReplaceDeferred {
                state_id,
                owner,
                expected_revision,
                content,
                result,
            } => {
                let replaced = match content.await {
                    Ok((payload, new_records)) => {
                        store
                            .replace(&state_id, &owner, expected_revision, &payload, &new_records)
                            .await
                    }
                    Err(error) => Err(error),
                };
                drop(result.send(replaced));
            }
            Command::Commit {
                state_id,
                owner,
                expected_revision,
                payload,
                records: new_records,
                events,
                summary,
                result,
            } => {
                drop(
                    result.send(
                        store
                            .commit(
                                &state_id,
                                &owner,
                                expected_revision,
                                StateCommit {
                                    payload: &payload,
                                    records: &new_records,
                                    events: &events,
                                    summary: &summary,
                                },
                            )
                            .await,
                    ),
                );
            }
            #[cfg(not(target_family = "wasm"))]
            Command::AcquireWith {
                state_id,
                owner_id,
                update,
                result,
            } => {
                drop(result.send(store.acquire_with(&state_id, owner_id, update).await));
            }
            #[cfg(not(target_family = "wasm"))]
            Command::AcquireWithCompanion {
                state_id,
                owner_id,
                update,
                companion_id,
                companion_owner,
                result,
            } => {
                drop(
                    result.send(
                        store
                            .acquire_with_companion(
                                &state_id,
                                owner_id,
                                update,
                                &companion_id,
                                companion_owner,
                            )
                            .await,
                    ),
                );
            }
            Command::Peek { state_id, result } => {
                drop(result.send(store.peek(&state_id).await));
            }
            Command::Owner { state_id, result } => {
                drop(result.send(store.owner(&state_id).await));
            }
            Command::ListStates { limit, result } => {
                drop(result.send(store.list_states(limit).await));
            }
            Command::ListRecords { state_id, result } => {
                drop(result.send(store.list_records(&state_id).await));
            }
            Command::ListSummaries { limit, result } => {
                drop(result.send(store.list_summaries(limit).await));
            }
            Command::RememberSummaries { summaries, result } => {
                drop(result.send(store.remember_summaries(summaries).await));
            }
            Command::Flush { result } => {
                drop(result.send(store.flush().await));
            }
        }
    }
    .instrument(span)
    .await;
}

impl SharedStore {
    pub(crate) fn replace_latest(
        &self,
        state_id: &str,
        owner: &OwnerToken,
        expected_revision: u64,
        write: std::sync::Arc<LatestWrite>,
    ) -> Result<oneshot::Receiver<Result<u64, StoreError>>, StoreError> {
        let (result, receiver) = oneshot::channel();
        self.commands
            .send(StoreRequest {
                command: Command::ReplaceLatest {
                    state_id: state_id.to_owned(),
                    owner: owner.clone(),
                    expected_revision,
                    write,
                    result,
                },
                span: tracing::info_span!(
                    "durability.store",
                    operation = "replace_latest",
                    state.id = state_id,
                    queue.duration_ms = tracing::field::Empty
                ),
                queued: Instant::now(),
            })
            .map_err(|_| stopped())?;
        Ok(receiver)
    }
}

impl SharedStore {
    pub(crate) fn replace_deferred(
        &self,
        state_id: &str,
        owner: &OwnerToken,
        expected_revision: u64,
        content: DeferredContent,
    ) -> Result<oneshot::Receiver<Result<u64, StoreError>>, StoreError> {
        let (result, receiver) = oneshot::channel();
        self.commands
            .send(StoreRequest {
                command: Command::ReplaceDeferred {
                    state_id: state_id.to_owned(),
                    owner: owner.clone(),
                    expected_revision,
                    content,
                    result,
                },
                span: tracing::info_span!(
                    "durability.store",
                    operation = "replace_deferred",
                    state.id = state_id,
                    queue.duration_ms = tracing::field::Empty
                ),
                queued: Instant::now(),
            })
            .map_err(|_| stopped())?;
        Ok(receiver)
    }
}

pub(crate) fn latest_write(payload: String, records: Vec<crate::StoreRecord>) -> LatestWrite {
    LatestWrite(nanocode_sync::Atom::new(Latest::Open {
        payload: std::sync::Arc::from(payload),
        keys: records.iter().map(|record| record.key.clone()).collect(),
        records: records.into_iter().collect(),
    }))
}

impl StateStore for SharedStore {
    fn flush(&mut self) -> StoreFuture<'_, Result<(), StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "flush",
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::Flush { result },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn batches_companion_acquisition(&self) -> bool {
        self.batches_companion_acquisition
    }

    fn supports_events(&self) -> bool {
        self.supports_events
    }

    fn supports_summaries(&self) -> bool {
        self.supports_summaries
    }

    fn commit<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        commit: StateCommit<'a>,
    ) -> StoreFuture<'a, Result<Committed, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "commit",
            state.id = state_id,
            payload.bytes = commit.payload.len(),
            record.count = commit.records.len(),
            event.count = commit.events.len(),
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::Commit {
                        state_id: state_id.to_owned(),
                        owner: owner.clone(),
                        expected_revision,
                        payload: commit.payload.to_owned(),
                        records: commit.records.to_vec(),
                        events: commit.events.to_vec(),
                        summary: commit.summary.clone(),
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    #[cfg(not(target_family = "wasm"))]
    fn acquire_with<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "acquire_with",
            state.id = state_id,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::AcquireWith {
                        state_id: state_id.to_owned(),
                        owner_id,
                        update,
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    #[cfg(not(target_family = "wasm"))]
    fn acquire_with_companion<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
        update: StoreUpdate,
        companion_id: &'a str,
        companion_owner: OwnerId,
    ) -> StoreFuture<'a, Result<(OwnedState, OwnedState), StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "acquire_with_companion",
            state.id = state_id,
            companion.id = companion_id,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::AcquireWithCompanion {
                        state_id: state_id.to_owned(),
                        owner_id,
                        update,
                        companion_id: companion_id.to_owned(),
                        companion_owner,
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "read_records",
            state.id = state_id,
            record.count = keys.len(),
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::ReadRecords {
                        state_id: state_id.to_owned(),
                        keys: keys.to_vec(),
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "read_record",
            state.id = state_id,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::ReadRecord {
                        state_id: state_id.to_owned(),
                        key: key.to_owned(),
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "acquire",
            state.id = state_id,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::Acquire {
                        state_id: state_id.to_owned(),
                        owner_id,
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [crate::StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "replace",
            state.id = state_id,
            payload.bytes = payload.len(),
            record.count = records.len(),
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::Replace {
                        state_id: state_id.to_owned(),
                        owner: owner.clone(),
                        expected_revision,
                        payload: payload.to_owned(),
                        records: records.to_vec(),
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<crate::StoredState, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "peek",
            state.id = state_id,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::Peek {
                        state_id: state_id.to_owned(),
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn owner<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "owner",
            state.id = state_id,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::Owner {
                        state_id: state_id.to_owned(),
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "list_states",
            state.limit = limit,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::ListStates { limit, result },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn list_records<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "list_records",
            state.id = state_id,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::ListRecords {
                        state_id: state_id.to_owned(),
                        result,
                    },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn list_summaries<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Option<Vec<ListedState>>, StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "list_summaries",
            state.limit = limit,
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::ListSummaries { limit, result },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }

    fn remember_summaries<'a>(
        &'a mut self,
        summaries: Vec<KnownSummary>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        let span = tracing::info_span!(
            "durability.store",
            operation = "remember_summaries",
            state.count = summaries.len(),
            queue.duration_ms = tracing::field::Empty
        );
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.commands
                .send(StoreRequest {
                    command: Command::RememberSummaries { summaries, result },
                    span,
                    queued: Instant::now(),
                })
                .map_err(|_| stopped())?;
            receiver.await.map_err(|_| stopped())?
        })
    }
}

enum ReadCommand {
    State {
        state_id: String,
        result: oneshot::Sender<Result<StoredState, StoreError>>,
    },
    Records {
        state_id: String,
        keys: Vec<String>,
        result: oneshot::Sender<Result<Vec<Option<String>>, StoreError>>,
    },
    Events {
        state_id: String,
        range: EventRange,
        result: oneshot::Sender<Result<EventPage, StoreError>>,
    },
    States {
        query: StateQuery,
        result: oneshot::Sender<Result<StatePage, StoreError>>,
    },
    Hits {
        query: EventQuery,
        result: oneshot::Sender<Result<EventHits, StoreError>>,
    },
}

#[derive(Clone)]
pub(crate) struct SharedReader {
    commands: mpsc::Sender<ReadCommand>,
}

impl SharedReader {
    pub(crate) fn new<R>(mut reader: R) -> crate::Result<Self>
    where
        R: StateReader + 'static,
    {
        let (commands, mut receiver) = mpsc::channel(COMMAND_CAPACITY);
        spawn_driver(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    ReadCommand::State { state_id, result } => {
                        drop(result.send(reader.read_state(&state_id).await));
                    }
                    ReadCommand::Records {
                        state_id,
                        keys,
                        result,
                    } => {
                        drop(result.send(reader.read_records(&state_id, &keys).await));
                    }
                    ReadCommand::Events {
                        state_id,
                        range,
                        result,
                    } => {
                        drop(result.send(reader.read_events(&state_id, &range).await));
                    }
                    ReadCommand::States { query, result } => {
                        drop(result.send(reader.query_states(&query).await));
                    }
                    ReadCommand::Hits { query, result } => {
                        drop(result.send(reader.query_events(&query).await));
                    }
                }
            }
        })?;
        Ok(Self { commands })
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, StoreError>>) -> ReadCommand,
    ) -> Result<T, StoreError> {
        let (result, receiver) = oneshot::channel();
        self.commands
            .send(command(result))
            .await
            .map_err(|_| stopped())?;
        receiver.await.map_err(|_| stopped())?
    }
}

impl StateReader for SharedReader {
    fn read_state<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        Box::pin(self.request(|result| ReadCommand::State {
            state_id: state_id.to_owned(),
            result,
        }))
    }

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(self.request(|result| ReadCommand::Records {
            state_id: state_id.to_owned(),
            keys: keys.to_vec(),
            result,
        }))
    }

    fn read_events<'a>(
        &'a mut self,
        state_id: &'a str,
        range: &'a EventRange,
    ) -> StoreFuture<'a, Result<EventPage, StoreError>> {
        Box::pin(self.request(|result| ReadCommand::Events {
            state_id: state_id.to_owned(),
            range: range.clone(),
            result,
        }))
    }

    fn query_states<'a>(
        &'a mut self,
        query: &'a StateQuery,
    ) -> StoreFuture<'a, Result<StatePage, StoreError>> {
        Box::pin(self.request(|result| ReadCommand::States {
            query: query.clone(),
            result,
        }))
    }

    fn query_events<'a>(
        &'a mut self,
        query: &'a EventQuery,
    ) -> StoreFuture<'a, Result<EventHits, StoreError>> {
        Box::pin(self.request(|result| ReadCommand::Hits {
            query: query.clone(),
            result,
        }))
    }
}

fn stopped() -> StoreError {
    StoreError::Backend("shared durability store stopped".to_owned())
}

#[cfg(not(target_family = "wasm"))]
fn spawn_driver(driver: impl Future<Output = ()> + Send + 'static) -> crate::Result<()> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::RuntimeUnavailable)?;
    drop(runtime.spawn(driver));
    Ok(())
}

#[cfg(target_family = "wasm")]
fn spawn_driver(driver: impl Future<Output = ()> + 'static) -> crate::Result<()> {
    nanocode_platform::spawn_local(driver);
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn spawn_operation(operation: impl Future<Output = ()> + Send + 'static) {
    drop(tokio::spawn(operation));
}

#[cfg(target_family = "wasm")]
fn spawn_operation(operation: impl Future<Output = ()> + 'static) {
    nanocode_platform::spawn_local(operation);
}
