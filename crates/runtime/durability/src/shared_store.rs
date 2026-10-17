use std::future::Future;

use nanocode_platform::Instant;
use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;

use crate::{
    Committed, EventHits, EventPage, EventQuery, EventRange, OwnedState, OwnerId, OwnerToken,
    StateCommit, StatePage, StateQuery, StateReader, StateStore, StoreError, StoreEvent,
    StoreFuture, StoredState, SummaryUpdate,
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
    ListStates {
        limit: usize,
        result: oneshot::Sender<Result<Vec<String>, StoreError>>,
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

    pub(crate) fn new<S>(mut store: S) -> crate::Result<Self>
    where
        S: StateStore + 'static,
    {
        #[cfg(not(target_family = "wasm"))]
        let batches_companion_acquisition = store.batches_companion_acquisition();
        #[cfg(target_family = "wasm")]
        let batches_companion_acquisition = false;
        let supports_events = store.supports_events();
        let (commands, mut receiver) = mpsc::channel(COMMAND_CAPACITY);
        spawn_driver(async move {
            while let Some(StoreRequest {
                command,
                span,
                queued,
            }) = receiver.recv().await
            {
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
                                        .replace(
                                            &state_id,
                                            &owner,
                                            expected_revision,
                                            &payload,
                                            &new_records,
                                        )
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
                            drop(
                                result.send(store.acquire_with(&state_id, owner_id, update).await),
                            );
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
                        Command::ListStates { limit, result } => {
                            drop(result.send(store.list_states(limit).await));
                        }
                        Command::Flush { result } => {
                            drop(result.send(store.flush().await));
                        }
                    }
                }
                .instrument(span)
                .await;
            }
        })?;
        Ok(Self {
            commands,
            batches_companion_acquisition,
            supports_events,
        })
    }
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
