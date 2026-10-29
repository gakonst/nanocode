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

struct StoreRequest {
    command: Command,
    span: tracing::Span,
    queued: Instant,
}

#[derive(Clone)]
pub struct SharedStore {
    commands: mpsc::Sender<StoreRequest>,
    batches_companion_acquisition: bool,
    supports_events: bool,
    supports_summaries: bool,
    pub(crate) terminal_receipt_limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct WeakSharedStore(mpsc::WeakSender<StoreRequest>);

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
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
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

async fn drive_grouped<S: StateStore>(mut store: S, mut receiver: mpsc::Receiver<StoreRequest>) {
    let mut next = None;
    loop {
        let request = match next.take() {
            Some(request) => request,
            None => match receiver.recv().await {
                Some(request) => request,
                None => break,
            },
        };
        let first = match Grouped::from_request(request) {
            Ok(grouped) => grouped,
            Err(request) => {
                run(&mut store, request).await;
                continue;
            }
        };
        let mut group = vec![first];
        while let Ok(request) = receiver.try_recv() {
            match Grouped::from_request(request) {
                Ok(grouped)
                    if group
                        .iter()
                        .all(|other| other.commit.state_id != grouped.commit.state_id) =>
                {
                    group.push(grouped);
                }
                Ok(grouped) => {
                    next = Some(grouped.into_request());
                    break;
                }
                Err(request) => {
                    next = Some(request);
                    break;
                }
            }
        }
        match group.pop() {
            Some(single) if group.is_empty() => {
                run(&mut store, single.into_request()).await;
            }
            Some(last) => {
                group.push(last);
                run_group(&mut store, group).await;
            }
            None => {}
        }
    }
}

struct Completion(mpsc::UnboundedSender<()>);

impl Drop for Completion {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

async fn drive_concurrently<S: StateStore>(
    mut store: S,
    mut receiver: mpsc::Receiver<StoreRequest>,
) {
    let (finished, mut completions) = mpsc::unbounded_channel();
    let mut in_flight = 0_usize;
    loop {
        let request = tokio::select! {
            biased;
            Some(()) = completions.recv(), if in_flight > 0 => {
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
        let completion = Completion(finished.clone());
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

struct Grouped {
    commit: OwnedCommit,
    reply: Reply,
    span: tracing::Span,
    queued: Instant,
}

impl Grouped {
    fn from_request(request: StoreRequest) -> Result<Self, StoreRequest> {
        let StoreRequest {
            command,
            span,
            queued,
        } = request;
        let (commit, reply) = match command {
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
            commit,
            reply,
            span,
            queued,
        })
    }

    fn into_request(self) -> StoreRequest {
        let OwnedCommit {
            state_id,
            owner,
            expected_revision,
            payload,
            records,
            events,
            summary,
        } = self.commit;
        let command = match self.reply {
            Reply::Replace(result) => Command::Replace {
                state_id,
                owner,
                expected_revision,
                payload,
                records,
                result,
            },
            Reply::Commit(result) => Command::Commit {
                state_id,
                owner,
                expected_revision,
                payload,
                records,
                events,
                summary,
                result,
            },
        };
        StoreRequest {
            command,
            span: self.span,
            queued: self.queued,
        }
    }
}

async fn run_group<S: StateStore + ?Sized>(store: &mut S, group: Vec<Grouped>) {
    let count = group.len();
    let mut commits = Vec::with_capacity(count);
    let mut replies = Vec::with_capacity(count);
    for grouped in group {
        grouped.span.record(
            "queue.duration_ms",
            grouped.queued.elapsed().as_secs_f64() * 1_000.0,
        );
        commits.push(grouped.commit);
        replies.push(grouped.reply);
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
        for reply in replies {
            reply.send(Err(error.clone()));
        }
        return;
    }
    for (reply, result) in replies.into_iter().zip(results) {
        reply.send(result);
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
                .await
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
