use std::{collections::HashMap, future::Future};

use tokio::sync::{mpsc, oneshot};

#[cfg(not(target_family = "wasm"))]
use crate::Error;
use crate::{
    Committed, EventCursor, EventHit, EventHits, EventOrder, EventPage, EventQuery, EventRange,
    IndexedState, OwnedState, OwnerId, OwnerToken, StateCommit, StateCursor, StatePage, StateQuery,
    StateReader, StateStore, StateSummary, StoreError, StoreEvent, StoreFuture, StoredEvent,
    StoredState, SummaryUpdate,
};

const COMMAND_CAPACITY: usize = 64;

enum Command {
    ReadRecord {
        state_id: String,
        key: String,
        result: oneshot::Sender<Result<Option<String>, StoreError>>,
    },
    ReadRecords {
        state_id: String,
        keys: Vec<String>,
        result: oneshot::Sender<Result<Vec<Option<String>>, StoreError>>,
    },
    AcquireOwner {
        state_id: String,
        owner_id: OwnerId,
        result: oneshot::Sender<Result<OwnedState, StoreError>>,
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
        result: oneshot::Sender<Result<StoredState, StoreError>>,
    },
    ListStates {
        limit: usize,
        result: oneshot::Sender<Result<Vec<String>, StoreError>>,
    },
    ReadEvents {
        state_id: String,
        range: EventRange,
        result: oneshot::Sender<Result<EventPage, StoreError>>,
    },
    QueryStates {
        query: StateQuery,
        result: oneshot::Sender<Result<StatePage, StoreError>>,
    },
    QueryEvents {
        query: EventQuery,
        result: oneshot::Sender<Result<EventHits, StoreError>>,
    },
}

#[derive(Default)]
struct Tables {
    records: HashMap<(String, String), String>,
    states: HashMap<String, StoredState>,
    owners: HashMap<String, OwnerToken>,
    created: Vec<String>,
    events: HashMap<String, Vec<StoredEvent>>,
    summaries: HashMap<String, (u64, Option<StateSummary>)>,
}

impl Tables {
    fn acquire(&mut self, state_id: String, owner_id: OwnerId) -> Result<OwnedState, StoreError> {
        let fence = self
            .owners
            .get(&state_id)
            .map_or(Some(1), |owner| owner.fence().checked_add(1))
            .ok_or_else(|| {
                StoreError::NotCommitted("in-memory durability owner fence overflow".to_owned())
            })?;
        let owner = OwnerToken::new(owner_id, fence);
        self.owners.insert(state_id.clone(), owner.clone());
        Ok(OwnedState {
            owner,
            state: self.states.get(&state_id).cloned().unwrap_or_default(),
        })
    }

    fn commit(
        &mut self,
        state_id: &str,
        owner: &OwnerToken,
        expected_revision: u64,
        commit: StateCommit<'_>,
    ) -> Result<Committed, StoreError> {
        if self.owners.get(state_id) != Some(owner) {
            return Err(StoreError::Fenced);
        }
        let current = self.states.get(state_id).map_or(0, |state| state.revision);
        if current != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                actual: current,
            });
        }
        let revision = current.checked_add(1).ok_or_else(|| {
            StoreError::NotCommitted("in-memory durability revision overflow".to_owned())
        })?;
        let mut staged = HashMap::new();
        for record in commit.records {
            let retained = self
                .records
                .get(&(state_id.to_owned(), record.key.clone()))
                .or_else(|| staged.get(&record.key).copied());
            if retained.is_some_and(|value| value != &record.value) {
                return Err(StoreError::NotCommitted(format!(
                    "durability record `{}` has conflicting content",
                    record.key
                )));
            }
            staged.insert(record.key.clone(), &record.value);
        }
        let first_event = self
            .events
            .get(state_id)
            .and_then(|events| events.last())
            .map_or(Some(1), |last| last.seq.checked_add(1))
            .filter(|first| first.checked_add(commit.events.len() as u64).is_some())
            .ok_or_else(|| {
                StoreError::NotCommitted("in-memory durability event sequence overflow".to_owned())
            })?;

        for record in commit.records {
            self.records
                .entry((state_id.to_owned(), record.key.clone()))
                .or_insert_with(|| record.value.clone());
        }
        let state = self.states.entry(state_id.to_owned()).or_default();
        if state.payload.is_none() {
            self.created.push(state_id.to_owned());
        }
        state.payload = Some(commit.payload.to_owned());
        state.revision = revision;
        let events = self.events.entry(state_id.to_owned()).or_default();
        events.extend(
            (first_event..)
                .zip(commit.events)
                .map(|(seq, event)| StoredEvent {
                    seq,
                    revision,
                    event: event.clone(),
                }),
        );
        match commit.summary {
            SummaryUpdate::Unchanged => {}
            SummaryUpdate::Set(summary) => {
                self.summaries
                    .insert(state_id.to_owned(), (revision, Some(summary.clone())));
            }
            SummaryUpdate::Remove => {
                self.summaries.insert(state_id.to_owned(), (revision, None));
            }
        }
        Ok(Committed {
            revision,
            first_event: (!commit.events.is_empty()).then_some(first_event),
        })
    }

    fn read_events(&self, state_id: &str, range: &EventRange) -> EventPage {
        if range.limit == 0 {
            return EventPage::default();
        }
        let matching = self
            .events
            .get(state_id)
            .into_iter()
            .flatten()
            .filter(|event| {
                range.after.is_none_or(|after| event.seq > after)
                    && range.before.is_none_or(|before| event.seq < before)
                    && matches_kind(&range.kinds, &event.event)
            });
        let mut events: Vec<_> = match range.order {
            EventOrder::Ascending => matching
                .take(range.limit.saturating_add(1))
                .cloned()
                .collect(),
            EventOrder::Descending => matching
                .rev()
                .take(range.limit.saturating_add(1))
                .cloned()
                .collect(),
        };
        let more = events.len() > range.limit;
        events.truncate(range.limit);
        EventPage {
            next: more.then(|| events.last().map(|event| event.seq)).flatten(),
            events,
        }
    }

    fn query_states(&self, query: &StateQuery) -> StatePage {
        if query.limit == 0 {
            return StatePage::default();
        }
        let mut states: Vec<_> = self
            .summaries
            .iter()
            .filter_map(|(state_id, (revision, summary))| {
                let summary = summary.as_ref()?;
                let cursor = StateCursor {
                    updated_at_ms: summary.updated_at_ms,
                    state_id: state_id.clone(),
                };
                let listed = query.after.as_ref().is_none_or(|after| cursor < *after)
                    && matches_value(query.scope.as_ref(), summary.scope.as_ref())
                    && matches_value(query.parent.as_ref(), summary.parent.as_ref())
                    && query
                        .attributes
                        .iter()
                        .all(|(key, value)| summary.attributes.get(key) == Some(value))
                    && (query.text.is_empty()
                        || matches_text(&query.text, summary.title.as_deref())
                        || matches_text(&query.text, summary.preview.as_deref())
                        || self.events.get(state_id).is_some_and(|events| {
                            events
                                .iter()
                                .any(|event| matches_text(&query.text, event.event.text.as_deref()))
                        }));
                listed.then(|| {
                    (
                        cursor,
                        IndexedState {
                            state_id: state_id.clone(),
                            revision: *revision,
                            summary: summary.clone(),
                        },
                    )
                })
            })
            .collect();
        states.sort_unstable_by(|left, right| right.0.cmp(&left.0));
        let more = states.len() > query.limit;
        states.truncate(query.limit);
        StatePage {
            next: more
                .then(|| states.last().map(|(cursor, _)| cursor.clone()))
                .flatten(),
            states: states.into_iter().map(|(_, state)| state).collect(),
        }
    }

    fn query_events(&self, query: &EventQuery) -> EventHits {
        if query.limit == 0 {
            return EventHits::default();
        }
        let mut hits: Vec<_> = self
            .summaries
            .iter()
            .filter_map(|(state_id, (_, summary))| Some((state_id, summary.as_ref()?)))
            .filter(|(state_id, summary)| {
                matches_value(query.state_id.as_ref(), Some(*state_id))
                    && matches_value(query.scope.as_ref(), summary.scope.as_ref())
            })
            .flat_map(|(state_id, _)| {
                self.events
                    .get(state_id)
                    .into_iter()
                    .flatten()
                    .map(move |event| {
                        let cursor = EventCursor {
                            at_ms: event.event.at_ms,
                            state_id: state_id.clone(),
                            seq: event.seq,
                        };
                        (cursor, event)
                    })
            })
            .filter(|(cursor, event)| {
                query.after.as_ref().is_none_or(|after| cursor < after)
                    && matches_kind(&query.kinds, &event.event)
                    && (query.text.is_empty()
                        || matches_text(&query.text, event.event.text.as_deref()))
            })
            .collect();
        hits.sort_unstable_by(|left, right| right.0.cmp(&left.0));
        let more = hits.len() > query.limit;
        hits.truncate(query.limit);
        EventHits {
            next: more
                .then(|| hits.last().map(|(cursor, _)| cursor.clone()))
                .flatten(),
            hits: hits
                .into_iter()
                .map(|(cursor, event)| EventHit {
                    state_id: cursor.state_id,
                    event: event.clone(),
                })
                .collect(),
        }
    }
}

fn matches_value(expected: Option<&String>, actual: Option<&String>) -> bool {
    expected.is_none_or(|expected| actual == Some(expected))
}

fn matches_kind(kinds: &[String], event: &StoreEvent) -> bool {
    kinds.is_empty() || kinds.contains(&event.kind)
}

fn matches_text(patterns: &[String], text: Option<&str>) -> bool {
    let Some(text) = text else {
        return false;
    };
    patterns.iter().any(|pattern| {
        pattern.is_empty()
            || text
                .as_bytes()
                .windows(pattern.len())
                .any(|window| window.eq_ignore_ascii_case(pattern.as_bytes()))
    })
}

#[derive(Clone)]
pub struct MemoryStore {
    commands: mpsc::Sender<Command>,
}

impl MemoryStore {
    pub fn new() -> crate::Result<Self> {
        let (commands, mut receiver) = mpsc::channel(COMMAND_CAPACITY);
        let driver = async move {
            let mut tables = Tables::default();
            while let Some(command) = receiver.recv().await {
                match command {
                    Command::ReadRecord {
                        state_id,
                        key,
                        result,
                    } => {
                        drop(result.send(Ok(tables.records.get(&(state_id, key)).cloned())));
                    }
                    Command::ReadRecords {
                        state_id,
                        keys,
                        result,
                    } => {
                        drop(
                            result.send(Ok(keys
                                .into_iter()
                                .map(|key| tables.records.get(&(state_id.clone(), key)).cloned())
                                .collect())),
                        );
                    }
                    Command::AcquireOwner {
                        state_id,
                        owner_id,
                        result,
                    } => {
                        drop(result.send(tables.acquire(state_id, owner_id)));
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
                    } => {
                        drop(result.send(tables.commit(
                            &state_id,
                            &owner,
                            expected_revision,
                            StateCommit {
                                payload: &payload,
                                records: &records,
                                events: &events,
                                summary: &summary,
                            },
                        )));
                    }
                    Command::Peek { state_id, result } => {
                        drop(result.send(Ok(
                            tables.states.get(&state_id).cloned().unwrap_or_default(),
                        )));
                    }
                    Command::ListStates { limit, result } => {
                        drop(result.send(Ok(
                            tables.created.iter().rev().take(limit).cloned().collect(),
                        )));
                    }
                    Command::ReadEvents {
                        state_id,
                        range,
                        result,
                    } => {
                        drop(result.send(Ok(tables.read_events(&state_id, &range))));
                    }
                    Command::QueryStates { query, result } => {
                        drop(result.send(Ok(tables.query_states(&query))));
                    }
                    Command::QueryEvents { query, result } => {
                        drop(result.send(Ok(tables.query_events(&query))));
                    }
                }
            }
        };
        spawn_driver(driver)?;
        Ok(Self { commands })
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, StoreError>>) -> Command,
    ) -> Result<T, StoreError> {
        let (result, receiver) = oneshot::channel();
        self.commands
            .send(command(result))
            .await
            .map_err(|_| stopped())?;
        receiver.await.map_err(|_| stopped())?
    }
}

impl StateStore for MemoryStore {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        Box::pin(self.request(|result| Command::ReadRecord {
            state_id: state_id.to_owned(),
            key: key.to_owned(),
            result,
        }))
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        Box::pin(self.request(|result| Command::AcquireOwner {
            state_id: state_id.to_owned(),
            owner_id,
            result,
        }))
    }

    fn replace<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        payload: &'a str,
        records: &'a [crate::StoreRecord],
    ) -> StoreFuture<'a, Result<u64, StoreError>> {
        Box::pin(async move {
            let committed = self
                .request(|result| Command::Commit {
                    state_id: state_id.to_owned(),
                    owner: owner.clone(),
                    expected_revision,
                    payload: payload.to_owned(),
                    records: records.to_vec(),
                    events: Vec::new(),
                    summary: SummaryUpdate::Unchanged,
                    result,
                })
                .await?;
            Ok(committed.revision)
        })
    }

    fn supports_events(&self) -> bool {
        true
    }

    fn commit<'a>(
        &'a mut self,
        state_id: &'a str,
        owner: &'a OwnerToken,
        expected_revision: u64,
        commit: StateCommit<'a>,
    ) -> StoreFuture<'a, Result<Committed, StoreError>> {
        Box::pin(self.request(move |result| Command::Commit {
            state_id: state_id.to_owned(),
            owner: owner.clone(),
            expected_revision,
            payload: commit.payload.to_owned(),
            records: commit.records.to_vec(),
            events: commit.events.to_vec(),
            summary: commit.summary.clone(),
            result,
        }))
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        Box::pin(self.request(|result| Command::Peek {
            state_id: state_id.to_owned(),
            result,
        }))
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(self.request(move |result| Command::ListStates { limit, result }))
    }
}

impl StateReader for MemoryStore {
    fn read_state<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        Box::pin(self.request(|result| Command::Peek {
            state_id: state_id.to_owned(),
            result,
        }))
    }

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(self.request(|result| Command::ReadRecords {
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
        Box::pin(self.request(|result| Command::ReadEvents {
            state_id: state_id.to_owned(),
            range: range.clone(),
            result,
        }))
    }

    fn query_states<'a>(
        &'a mut self,
        query: &'a StateQuery,
    ) -> StoreFuture<'a, Result<StatePage, StoreError>> {
        Box::pin(self.request(|result| Command::QueryStates {
            query: query.clone(),
            result,
        }))
    }

    fn query_events<'a>(
        &'a mut self,
        query: &'a EventQuery,
    ) -> StoreFuture<'a, Result<EventHits, StoreError>> {
        Box::pin(self.request(|result| Command::QueryEvents {
            query: query.clone(),
            result,
        }))
    }
}

fn stopped() -> StoreError {
    StoreError::Backend("in-memory durability store stopped".to_owned())
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
