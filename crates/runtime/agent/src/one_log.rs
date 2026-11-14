use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use nanocode_subagents::{AgentUpdate, ScopedAgentUpdate};
use nanocode_sync::Atom;
use nanocode_tool_host::{
    ToolInput, ToolOutput,
    code_mode::{
        CellJournal, CellJournalSink, CellJournalUpdate, CellJournals, JournalEntry,
        JournaledCallState,
    },
    effects::{EffectCall, EffectGate},
    embedded::HostFuture,
};
use nanocode_turnlog::{
    EffectClass, Effects, Fold, LogHost, Outcome, Phase, Point, Record, Stats, Status, Turn,
};
use serde::Serialize;
use serde_json::{Map, Value, json, value::RawValue};

use crate::{
    SessionEvent,
    durability::{
        Committed, OwnedState, OwnerId, OwnerToken, Settle, StateCommit, StateStore, StoreError,
        StoreFuture, StoreRecord, StoredState,
    },
    events::AgentEventKind,
    input::Prompt,
    session_log::{
        ImportedTurn, LogEntry, LogPage, LoggedEvent, Replaced, Started, Submission, Submitted,
        TurnRecord, TurnRetry, TurnState, edited, now, submission_digest,
    },
    stored_event::Stored,
    turn_inbox::Envelope,
    turn_log::{EventLog, Logged, Recording, TurnLog},
};

const TOOLS_MARK: &str = "$nanocode_tools";
const STORED_MARK: &str = "$nanocode_stored";

#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub effects: Effects,
    pub max_row_bytes: Option<usize>,
    pub restarts: u64,
    pub retain_finished: Option<usize>,
    pub snapshot_ratio: usize,
    pub snapshot_rows: Option<usize>,
}

pub struct OneLog {
    answered: Atom<BTreeMap<(String, u64), u64>>,
    dispatched: Atom<BTreeMap<(String, u64), BTreeSet<u64>>>,
    durable_settles: Atom<bool>,
    effects: Atom<Effects>,
    live: Atom<BTreeSet<(String, u64)>>,
    log: nanocode_turnlog::TurnLog,
    owners: Atom<BTreeMap<String, OwnerToken>>,
    recovering: Atom<BTreeSet<String>>,
    restarts: u64,
    unsaved: Atom<BTreeMap<(String, u64), u64>>,
}

#[derive(Clone)]
pub struct OneLogStore(Arc<OneLog>);

pub struct OneLogJournals(Arc<OneLog>);

#[derive(Serialize)]
struct Lifecycle<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: &'a str,
}

#[derive(Serialize)]
struct Finished<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: &'a str,
    status: &'static str,
    #[serde(flatten)]
    fields: &'a Map<String, Value>,
}

#[derive(Serialize)]
struct Replacement<'a> {
    digest: &'a str,
    edit_id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: &'a str,
}

#[derive(Serialize)]
struct Retrying<'a> {
    #[serde(rename = "type")]
    tag: &'static str,
    request_id: &'a str,
    error: &'a str,
    kind: &'a str,
    attempts: u64,
    retry_at: u64,
}

fn encoding(error: serde_json::Error) -> StoreError {
    StoreError::NotCommitted(format!("encode turn log record: {error}"))
}

fn raw(value: &impl Serialize) -> Result<Box<RawValue>, StoreError> {
    serde_json::value::to_raw_value(value).map_err(encoding)
}

fn invalid(detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("turn log state is invalid: {detail}"))
}

const fn outcome(state: TurnState) -> Option<Outcome> {
    match state {
        TurnState::Cancelled => Some(Outcome::Cancelled),
        TurnState::Completed => Some(Outcome::Completed),
        TurnState::Failed => Some(Outcome::Failed),
        TurnState::Queued | TurnState::Running => None,
    }
}

const fn telemetry(event: &SessionEvent) -> bool {
    let kind = match event {
        SessionEvent::Agent { event, .. } => event.kind,
        SessionEvent::Subagent(ScopedAgentUpdate {
            update: AgentUpdate::Event { event, .. },
            ..
        }) => event.kind,
        SessionEvent::Subagent(_) => return false,
    };
    matches!(
        kind,
        AgentEventKind::ApiEvent
            | AgentEventKind::ModelAttemptFailed
            | AgentEventKind::ModelAttemptRetrying
            | AgentEventKind::ModelAttemptStarted
            | AgentEventKind::ModelCallCompleted
            | AgentEventKind::ModelCallStarted
            | AgentEventKind::ModelConnectionCompleted
            | AgentEventKind::ModelConnectionFailed
            | AgentEventKind::ModelConnectionStarted
            | AgentEventKind::ModelWarmupCompleted
            | AgentEventKind::ModelWarmupFailed
            | AgentEventKind::ModelWarmupStarted
    )
}

fn retry_of(retry: Option<&nanocode_turnlog::Retry>) -> Option<TurnRetry> {
    retry.map(|retry| TurnRetry {
        attempts: retry.attempts,
        error: retry.error.clone(),
        kind: retry.kind.clone(),
        retry_at: retry.retry_at,
    })
}

fn record_of(turn: &Turn) -> Result<TurnRecord, StoreError> {
    let prompt = || {
        serde_json::from_str::<Prompt>(turn.prompt.get())
            .map(Some)
            .map_err(|error| invalid(format!("prompt of turn {}: {error}", turn.id)))
    };
    let (state, prompt, cancel_requested, note, last_seq, result, retry) = match &turn.phase {
        Phase::Queued { retry } => (
            TurnState::Queued,
            prompt()?,
            false,
            None,
            None,
            None,
            retry_of(retry.as_ref()),
        ),
        Phase::Running {
            cancelling,
            note,
            retry,
        } => (
            TurnState::Running,
            prompt()?,
            *cancelling,
            note.clone(),
            None,
            None,
            retry_of(retry.as_ref()),
        ),
        Phase::Finished {
            last,
            outcome,
            result,
        } => (
            match outcome {
                Outcome::Cancelled => TurnState::Cancelled,
                Outcome::Completed => TurnState::Completed,
                Outcome::Failed => TurnState::Failed,
            },
            None,
            false,
            None,
            Some(*last),
            Some(result.clone()),
            None,
        ),
    };
    Ok(TurnRecord {
        request_id: turn.id.clone(),
        state,
        digest: turn.digest.clone(),
        prompt,
        hand: turn.hand.clone(),
        metadata: turn.metadata.clone(),
        cancel_requested,
        note,
        first_seq: turn.first,
        last_seq,
        result,
        retry,
    })
}

fn logged(seq: u64, request_id: Option<&str>, at: u64, event: Box<RawValue>) -> LoggedEvent {
    LoggedEvent {
        seq,
        request_id: request_id.map(str::to_owned),
        created_at: at,
        event,
    }
}

fn stored_key(state: &str, key: &str) -> String {
    format!("{state}\n{key}")
}

impl OneLog {
    pub async fn open(
        host: Arc<dyn LogHost>,
        scope: impl Into<Arc<str>>,
        policy: Policy,
    ) -> Result<Arc<Self>, StoreError> {
        let log = nanocode_turnlog::TurnLog::open(host, scope)
            .await?
            .retain_finished(policy.retain_finished)
            .row_limit(policy.max_row_bytes)
            .snapshot_ratio(policy.snapshot_ratio)
            .snapshot_rows(policy.snapshot_rows);
        let recovering = log
            .read(|fold| {
                fold.turns()
                    .iter()
                    .filter(|turn| !matches!(turn.phase, Phase::Finished { .. }))
                    .map(|turn| turn.id.clone())
                    .collect()
            })
            .await;
        Ok(Arc::new(Self {
            answered: Atom::default(),
            dispatched: Atom::default(),
            durable_settles: Atom::new(false),
            effects: Atom::new(Effects::builtin().merged(policy.effects)),
            live: Atom::default(),
            log,
            owners: Atom::default(),
            recovering: Atom::new(recovering),
            restarts: policy.restarts,
            unsaved: Atom::default(),
        }))
    }

    #[must_use]
    pub fn scope(&self) -> &str {
        self.log.scope()
    }

    pub fn settle_durably(&self) {
        self.durable_settles.set(true);
    }

    pub fn declare(&self, tool: impl Into<String>, class: EffectClass) {
        let tool = tool.into();
        self.effects
            .update(|effects| effects.insert(tool.clone(), class));
    }

    #[must_use]
    pub fn effect_of(&self, tool: &str) -> EffectClass {
        self.effects.read(|effects| effects.of(tool))
    }

    pub async fn stats(&self) -> Stats {
        self.log.stats().await
    }

    pub async fn read<R>(&self, read: impl FnOnce(&Fold) -> R) -> R {
        self.log.read(read).await
    }

    #[must_use]
    pub fn store(self: &Arc<Self>) -> OneLogStore {
        OneLogStore(Arc::clone(self))
    }

    #[must_use]
    pub fn gate(self: &Arc<Self>) -> Arc<dyn EffectGate> {
        Arc::clone(self) as Arc<dyn EffectGate>
    }

    #[must_use]
    pub fn sink(self: &Arc<Self>) -> Arc<dyn CellJournalSink> {
        Arc::new(OneLogJournals(Arc::clone(self)))
    }

    pub async fn journals(self: &Arc<Self>) -> Result<CellJournals, StoreError> {
        let journals = CellJournals::with_sink(self.sink());
        for (session_id, cells) in self.cells(None).await? {
            journals.restore(&session_id, cells, None, HashMap::new());
        }
        Ok(journals)
    }

    async fn cells(
        &self,
        only: Option<&str>,
    ) -> Result<BTreeMap<String, Vec<CellJournal>>, StoreError> {
        self.log
            .read(|fold| {
                let mut sessions: BTreeMap<String, Vec<CellJournal>> = BTreeMap::new();
                for (session, _, updates) in fold.cells() {
                    if only.is_some_and(|only| only != session) {
                        continue;
                    }
                    let mut decoded = Vec::with_capacity(updates.len());
                    for update in updates {
                        let mut update = serde_json::from_str::<CellJournalUpdate>(update.get())
                            .map_err(|error| invalid(format!("cell journal update: {error}")))?;
                        if let Some(header) = update.header.as_mut()
                            && let [Value::Object(mark)] = &*header.tools
                            && let Some(digest) = mark.get(TOOLS_MARK).and_then(Value::as_str)
                        {
                            let catalog = fold
                                .stored(&stored_key(TOOLS_MARK, digest))
                                .ok_or_else(|| invalid(format!("tool catalog {digest}")))?;
                            header.tools = serde_json::from_str::<Vec<Value>>(catalog)
                                .map_err(|error| invalid(format!("tool catalog: {error}")))?
                                .into();
                        }
                        decoded.push(update);
                    }
                    if let Some(journal) = CellJournal::fold(decoded) {
                        sessions
                            .entry(session.to_owned())
                            .or_default()
                            .push(journal);
                    }
                }
                Ok(sessions)
            })
            .await
    }

    pub async fn field(&self, key: &str, value: Box<RawValue>) -> Result<(), StoreError> {
        self.log.lease().await.push(Record::Host {
            key: key.to_owned(),
            value,
        })
    }

    pub async fn effect(&self, key: &str, value: Box<RawValue>) -> Result<u64, StoreError> {
        let mut lease = self.log.lease().await;
        lease.push(Record::Host {
            key: key.to_owned(),
            value,
        })?;
        lease.commit(Point::Call).await
    }

    pub async fn submit(&self, submission: &Submission) -> Result<Submitted, StoreError> {
        self.admit(submission, true).await
    }

    pub async fn submit_buffered(&self, submission: &Submission) -> Result<Submitted, StoreError> {
        self.admit(submission, false).await
    }

    async fn admit(&self, submission: &Submission, durable: bool) -> Result<Submitted, StoreError> {
        let id = submission.request_id.as_str();
        let digest = submission_digest(&submission.prompt, submission.hand.as_deref())?;
        let mut lease = self.log.lease().await;
        if let Some(turn) = lease.fold().turn(id) {
            let record = record_of(turn)?;
            return Ok(if turn.digest == digest {
                Submitted::Existing(record)
            } else {
                Submitted::Conflict(record)
            });
        }
        if !durable {
            lease.secure().await?;
        }
        let first = lease.next_event();
        let at = now();
        let body = raw(&Lifecycle {
            kind: "queued",
            request_id: id,
        })?;
        lease.push(Record::Admitted {
            digest,
            first,
            hand: submission.hand.clone(),
            metadata: submission.metadata.clone(),
            prompt: raw(&submission.prompt)?,
            turn: id.to_owned(),
        })?;
        lease.push(Record::Event {
            at,
            event: body.clone(),
            seq: first,
            turn: Some(id.to_owned()),
        })?;
        if durable && let Err(error) = lease.commit(Point::Admission).await {
            lease.unadmit(id, first);
            return Err(error);
        }
        let record = record_of(
            lease
                .fold()
                .turn(id)
                .ok_or_else(|| invalid(format!("admitted turn {id} is missing")))?,
        )?;
        Ok(Submitted::Accepted {
            record,
            event: logged(first, Some(id), at, body),
        })
    }

    pub async fn append(&self, entries: &[LogEntry]) -> Result<Vec<LoggedEvent>, StoreError> {
        let mut lease = self.log.lease().await;
        lease.secure().await?;
        let mut events = Vec::with_capacity(entries.len());
        for entry in entries {
            let seq = lease.next_event();
            let at = now();
            lease.push(Record::Event {
                at,
                event: entry.event.clone(),
                seq,
                turn: entry.request_id.clone(),
            })?;
            events.push(logged(
                seq,
                entry.request_id.as_deref(),
                at,
                entry.event.clone(),
            ));
        }
        Ok(events)
    }

    pub async fn turn(&self, request_id: &str) -> Result<Option<TurnRecord>, StoreError> {
        self.log
            .read(|fold| fold.turn(request_id).map(record_of).transpose())
            .await
    }

    pub async fn pending(&self) -> Result<Vec<TurnRecord>, StoreError> {
        self.log
            .read(|fold| {
                fold.turns()
                    .iter()
                    .filter(|turn| !matches!(turn.phase, Phase::Finished { .. }))
                    .map(record_of)
                    .collect()
            })
            .await
    }

    async fn close(
        &self,
        lease: &mut nanocode_turnlog::Lease<'_>,
        request_id: &str,
        state: TurnState,
        fields: &Map<String, Value>,
    ) -> Result<(TurnRecord, LoggedEvent), StoreError> {
        let Some(outcome) = outcome(state) else {
            return Err(StoreError::NotCommitted(format!(
                "a turn cannot finish as {}",
                state.as_str()
            )));
        };
        let body = raw(&Finished {
            kind: "result",
            request_id,
            status: state.as_str(),
            fields,
        })?;
        let mut finished = lease
            .fold()
            .turn(request_id)
            .cloned()
            .ok_or_else(|| invalid(format!("finishing turn {request_id} is missing")))?;
        let last = lease.next_event();
        let at = now();
        lease.push(Record::Event {
            at,
            event: body.clone(),
            seq: last,
            turn: Some(request_id.to_owned()),
        })?;
        lease.push(Record::Status {
            status: Status::Finished {
                last,
                outcome,
                result: body.clone(),
            },
            turn: request_id.to_owned(),
        })?;
        finished.phase = Phase::Finished {
            last,
            outcome,
            result: body.clone(),
        };
        lease.commit(Point::Finish).await?;
        self.recovering.update(|turns| turns.remove(request_id));
        Ok((
            record_of(&finished)?,
            logged(last, Some(request_id), at, body),
        ))
    }

    pub async fn start(&self, request_id: &str) -> Result<Option<Started>, StoreError> {
        let mut lease = self.log.lease().await;
        let Some(turn) = lease.fold().turn(request_id) else {
            return Ok(None);
        };
        let previous = match turn.phase {
            Phase::Finished { .. } => return Ok(None),
            Phase::Queued { .. } => TurnState::Queued,
            Phase::Running { .. } => TurnState::Running,
        };
        let recovering = self.recovering.read(|turns| turns.contains(request_id));
        if recovering {
            let attempt = lease.fold().next_attempt(request_id);
            if attempt > self.restarts {
                let fields = Map::from_iter([(
                    "error".to_owned(),
                    Value::String(format!(
                        "the turn restarted {} times without progress",
                        attempt - 1
                    )),
                )]);
                self.close(&mut lease, request_id, TurnState::Failed, &fields)
                    .await?;
                return Ok(None);
            }
            let unknown = lease.fold().unknown();
            lease.push(Record::Recovery {
                at: None,
                attempt,
                turn: request_id.to_owned(),
                unknown,
            })?;
        } else {
            lease.secure().await?;
        }
        let seq = lease.next_event();
        let at = now();
        let body = raw(&Lifecycle {
            kind: "running",
            request_id,
        })?;
        lease.push(Record::Status {
            status: Status::Running,
            turn: request_id.to_owned(),
        })?;
        lease.push(Record::Event {
            at,
            event: body.clone(),
            seq,
            turn: Some(request_id.to_owned()),
        })?;
        if recovering {
            lease.commit(Point::Recovery).await?;
            self.recovering.update(|turns| turns.remove(request_id));
        }
        let record = record_of(
            lease
                .fold()
                .turn(request_id)
                .ok_or_else(|| invalid(format!("started turn {request_id} is missing")))?,
        )?;
        Ok(Some(Started {
            previous,
            record,
            event: logged(seq, Some(request_id), at, body),
        }))
    }

    pub async fn finish(
        &self,
        request_id: &str,
        state: TurnState,
        fields: &Map<String, Value>,
    ) -> Result<Option<(TurnRecord, LoggedEvent)>, StoreError> {
        let mut lease = self.log.lease().await;
        match lease.fold().turn(request_id).map(|turn| &turn.phase) {
            None | Some(Phase::Finished { .. }) => Ok(None),
            Some(_) => self
                .close(&mut lease, request_id, state, fields)
                .await
                .map(Some),
        }
    }

    pub async fn cancel(
        &self,
        request_id: &str,
        fields: &Map<String, Value>,
    ) -> Result<Option<(TurnRecord, Option<LoggedEvent>)>, StoreError> {
        let mut lease = self.log.lease().await;
        let Some(turn) = lease.fold().turn(request_id) else {
            return Ok(None);
        };
        match turn.phase {
            Phase::Running {
                cancelling: false, ..
            } => {
                lease.push(Record::Status {
                    status: Status::Cancelling,
                    turn: request_id.to_owned(),
                })?;
                lease.commit(Point::Cancel).await?;
                let record = lease.fold().turn(request_id).map(record_of).transpose()?;
                Ok(record.map(|record| (record, None)))
            }
            Phase::Queued { .. } => {
                let (record, event) = self
                    .close(&mut lease, request_id, TurnState::Cancelled, fields)
                    .await?;
                Ok(Some((record, Some(event))))
            }
            _ => Ok(Some((record_of(turn)?, None))),
        }
    }

    pub async fn withdraw_steered(
        &self,
        request_id: &str,
        digest: &str,
    ) -> Result<Option<(TurnRecord, LoggedEvent)>, StoreError> {
        let mut lease = self.log.lease().await;
        let queued = lease.fold().turn(request_id).is_some_and(|turn| {
            matches!(turn.phase, Phase::Queued { .. }) && turn.digest == digest
        });
        if !queued {
            return Ok(None);
        }
        let fields = Map::from_iter([
            ("cancelled".to_owned(), Value::Bool(true)),
            ("steered".to_owned(), Value::Bool(true)),
        ]);
        self.close(&mut lease, request_id, TurnState::Cancelled, &fields)
            .await
            .map(Some)
    }

    pub async fn interrupted(&self, request_id: &str, note: &str) -> Result<(), StoreError> {
        let mut lease = self.log.lease().await;
        let running = lease
            .fold()
            .turn(request_id)
            .is_some_and(|turn| matches!(turn.phase, Phase::Running { .. }));
        if !running {
            return Ok(());
        }
        lease.push(Record::Status {
            status: Status::Interrupted {
                note: note.to_owned(),
            },
            turn: request_id.to_owned(),
        })
    }

    pub async fn retry(
        &self,
        request_id: &str,
        error: &str,
        kind: &str,
        retry_at: u64,
    ) -> Result<Option<(TurnRecord, Option<LoggedEvent>)>, StoreError> {
        let mut lease = self.log.lease().await;
        let Some(turn) = lease.fold().turn(request_id) else {
            return Ok(None);
        };
        let previous = match &turn.phase {
            Phase::Finished { .. } => return Ok(Some((record_of(turn)?, None))),
            Phase::Queued { retry } | Phase::Running { retry, .. } => retry.clone(),
        };
        let attempts = previous.as_ref().map_or(0, |retry| retry.attempts) + 1;
        let changed = previous
            .as_ref()
            .is_none_or(|old| old.error != error || old.kind != kind);
        let event = if changed {
            lease.secure().await?;
            let seq = lease.next_event();
            let at = now();
            let body = raw(&Retrying {
                tag: "retrying",
                request_id,
                error,
                kind,
                attempts,
                retry_at,
            })?;
            lease.push(Record::Event {
                at,
                event: body.clone(),
                seq,
                turn: Some(request_id.to_owned()),
            })?;
            Some(logged(seq, Some(request_id), at, body))
        } else {
            None
        };
        lease.push(Record::Status {
            status: Status::Retrying {
                attempts,
                error: error.to_owned(),
                kind: kind.to_owned(),
                retry_at,
            },
            turn: request_id.to_owned(),
        })?;
        let record = lease.fold().turn(request_id).map(record_of).transpose()?;
        Ok(record.map(|record| (record, event)))
    }

    pub async fn import(&self, turns: &[ImportedTurn]) -> Result<usize, StoreError> {
        if turns.is_empty() {
            return Ok(0);
        }
        let mut lease = self.log.lease().await;
        let mut imported = false;
        let mut highest = 0;
        for turn in turns {
            if lease.fold().turn(&turn.request_id).is_some() {
                continue;
            }
            let id = turn.request_id.as_str();
            let first = turn.first_seq.unwrap_or(0);
            lease.push(Record::Admitted {
                digest: submission_digest(&turn.prompt, turn.hand.as_deref())?,
                first,
                hand: turn.hand.clone(),
                metadata: turn.metadata.clone(),
                prompt: raw(&turn.prompt)?,
                turn: id.to_owned(),
            })?;
            let status = |status| Record::Status {
                status,
                turn: id.to_owned(),
            };
            match outcome(turn.state) {
                Some(outcome) => {
                    let result = raw(&Finished {
                        kind: "result",
                        request_id: id,
                        status: turn.state.as_str(),
                        fields: turn.result.as_ref().unwrap_or(&Map::new()),
                    })?;
                    let last = match turn.last_seq {
                        Some(last) => last,
                        None => {
                            let last = lease.next_event();
                            lease.push(Record::Event {
                                at: now(),
                                event: result.clone(),
                                seq: last,
                                turn: Some(id.to_owned()),
                            })?;
                            last
                        }
                    };
                    highest = highest.max(last);
                    lease.push(status(Status::Finished {
                        last,
                        outcome,
                        result,
                    }))?;
                }
                None if turn.state == TurnState::Running => {
                    lease.push(status(Status::Running))?;
                    if turn.cancel_requested {
                        lease.push(status(Status::Cancelling))?;
                    }
                    if let Some(note) = &turn.note {
                        lease.push(status(Status::Interrupted { note: note.clone() }))?;
                    }
                }
                None => {}
            }
            highest = highest.max(first);
            imported = true;
        }
        if highest > lease.fold().last_event() {
            lease.push(Record::Floor { seq: highest })?;
        }
        if imported {
            lease.commit(Point::Admission).await?;
        }
        Ok(turns.len())
    }

    pub async fn import_events(
        &self,
        after: u64,
        events: &[LoggedEvent],
    ) -> Result<u64, StoreError> {
        let Some(last) = events.last() else {
            return Ok(after);
        };
        let mut previous = after;
        for event in events {
            if event.seq <= previous {
                return Err(StoreError::NotCommitted(format!(
                    "imported session log events must ascend after seq {previous}, but seq {} follows it",
                    event.seq
                )));
            }
            previous = event.seq;
        }
        let mut lease = self.log.lease().await;
        let ends = lease.fold().ends();
        if ends < after {
            return Err(StoreError::NotCommitted(format!(
                "turn log {} ends at seq {ends}, before the seq {after} the imported events follow",
                self.scope()
            )));
        }
        let held = lease.fold().events();
        let start = held.partition_point(|event| event.seq <= after);
        for event in &held[start..] {
            match events.iter().find(|import| import.seq == event.seq) {
                None => {
                    return Err(StoreError::NotCommitted(format!(
                        "turn log {} holds event {}, which the imported events after seq {after} do not",
                        self.scope(),
                        event.seq
                    )));
                }
                Some(import)
                    if import.request_id != event.turn
                        || import.created_at != event.at
                        || import.event.get() != event.event.get() =>
                {
                    return Err(StoreError::NotCommitted(format!(
                        "turn log {} already holds a different event at seq {}",
                        self.scope(),
                        event.seq
                    )));
                }
                Some(_) => {}
            }
        }
        let fresh = &events[events.partition_point(|event| event.seq <= ends)..];
        if fresh.is_empty() {
            return Ok(last.seq);
        }
        for event in fresh {
            lease.push(Record::Event {
                at: event.created_at,
                event: event.event.clone(),
                seq: event.seq,
                turn: event.request_id.clone(),
            })?;
        }
        lease.commit(Point::Admission).await?;
        Ok(last.seq)
    }

    pub async fn replace_queued(
        &self,
        request_id: &str,
        edit_id: &str,
        text: &str,
    ) -> Result<Replaced, StoreError> {
        if edit_id.is_empty() {
            return Err(StoreError::NotCommitted(
                "an edit ID must not be empty".into(),
            ));
        }
        let edit = submission_digest(&Prompt::new(text), None)?;
        let mut lease = self.log.lease().await;
        let Some(turn) = lease.fold().turn(request_id) else {
            return Ok(Replaced::NotQueued);
        };
        if let Some(applied) = turn.edits.get(edit_id) {
            return Ok(if *applied == edit {
                Replaced::Applied(None)
            } else {
                Replaced::Conflict
            });
        }
        if !matches!(turn.phase, Phase::Queued { .. }) {
            return Ok(Replaced::NotQueued);
        }
        let prompt = serde_json::from_str::<Prompt>(turn.prompt.get())
            .map_err(|error| invalid(format!("prompt of turn {request_id}: {error}")))?;
        let prompt = edited(&prompt, text)?;
        let digest = submission_digest(&prompt, turn.hand.as_deref())?;
        let body = raw(&Replacement {
            digest: &edit,
            edit_id,
            kind: "replaced",
            request_id,
        })?;
        let seq = lease.next_event();
        let at = now();
        lease.push(Record::Event {
            at,
            event: body.clone(),
            seq,
            turn: Some(request_id.to_owned()),
        })?;
        lease.push(Record::Status {
            status: Status::Replaced {
                digest,
                edit,
                edit_id: edit_id.to_owned(),
                prompt: raw(&prompt)?,
            },
            turn: request_id.to_owned(),
        })?;
        lease.commit(Point::Admission).await?;
        Ok(Replaced::Applied(Some(logged(
            seq,
            Some(request_id),
            at,
            body,
        ))))
    }

    pub async fn page(&self, after: u64, limit: usize) -> Result<LogPage, StoreError> {
        self.paged(None, after, limit, None).await
    }

    pub async fn page_bounded(
        &self,
        after: u64,
        limit: usize,
        max_bytes: usize,
    ) -> Result<LogPage, StoreError> {
        self.paged(None, after, limit, Some(max_bytes)).await
    }

    pub async fn turn_page(
        &self,
        request_id: &str,
        after: u64,
        limit: usize,
    ) -> Result<LogPage, StoreError> {
        self.paged(Some(request_id), after, limit, None).await
    }

    pub async fn turn_page_bounded(
        &self,
        request_id: &str,
        after: u64,
        limit: usize,
        max_bytes: usize,
    ) -> Result<LogPage, StoreError> {
        self.paged(Some(request_id), after, limit, Some(max_bytes))
            .await
    }

    async fn paged(
        &self,
        request_id: Option<&str>,
        after: u64,
        limit: usize,
        max_bytes: Option<usize>,
    ) -> Result<LogPage, StoreError> {
        let mut events = Vec::new();
        let mut bytes = 0;
        let mut has_more = false;
        let start = match request_id {
            Some(id) => {
                self.log
                    .read(|fold| {
                        fold.turn(id)
                            .map_or(after, |turn| after.max(turn.first.saturating_sub(1)))
                    })
                    .await
            }
            None => after,
        };
        self.log
            .history(start, |event| {
                if request_id.is_some_and(|id| event.turn.as_deref() != Some(id)) {
                    return true;
                }
                let size = event.event.get().len();
                if events.len() == limit
                    || max_bytes.is_some_and(|max| !events.is_empty() && bytes + size > max)
                {
                    has_more = true;
                    return false;
                }
                bytes += size;
                events.push(logged(
                    event.seq,
                    event.turn.as_deref(),
                    event.at,
                    event.event.clone(),
                ));
                true
            })
            .await?;
        Ok(LogPage {
            next_cursor: events.last().map_or(after, |event| event.seq),
            has_more,
            events,
        })
    }

    async fn recorded(&self, recording: Recording<'_>) -> Result<Vec<LoggedEvent>, String> {
        let mut lease = self.log.lease().await;
        lease.secure().await.map_err(|error| error.to_string())?;
        let mut logged_events = Vec::with_capacity(recording.events.len());
        for (event, request_id) in recording.events.iter().zip(recording.request_ids) {
            if telemetry(event) {
                let live = serde_json::value::to_raw_value(&Envelope {
                    event,
                    annotation: recording.annotation,
                })
                .map_err(|error| format!("encode agent event: {error}"))?;
                logged_events.push(logged(0, request_id.as_deref(), now(), live));
                continue;
            }
            let stored = serde_json::value::to_raw_value(&Stored {
                event,
                compact: None,
            })
            .map_err(|error| format!("encode agent event: {error}"))?;
            let live = serde_json::value::to_raw_value(&Envelope {
                event,
                annotation: recording.annotation,
            })
            .map_err(|error| format!("encode agent event: {error}"))?;
            let seq = lease.next_event();
            let at = now();
            lease
                .push(Record::Event {
                    at,
                    event: stored,
                    seq,
                    turn: request_id.clone(),
                })
                .map_err(|error| error.to_string())?;
            logged_events.push(logged(seq, request_id.as_deref(), at, live));
        }
        Ok(logged_events)
    }
}

impl EventLog for OneLog {
    fn record<'a>(
        &'a self,
        recording: Recording<'a>,
    ) -> HostFuture<'a, Result<Vec<LoggedEvent>, String>> {
        Box::pin(self.recorded(recording))
    }

    fn flush(&self) -> Logged<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl TurnLog for OneLog {
    fn turn<'a>(&'a self, request_id: &'a str) -> Logged<'a, Option<TurnRecord>> {
        Box::pin(Self::turn(self, request_id))
    }

    fn pending(&self) -> Logged<'_, Vec<TurnRecord>> {
        Box::pin(Self::pending(self))
    }

    fn start<'a>(&'a self, request_id: &'a str) -> Logged<'a, Option<Started>> {
        Box::pin(Self::start(self, request_id))
    }

    fn finish<'a>(
        &'a self,
        request_id: &'a str,
        state: TurnState,
        fields: &'a Map<String, Value>,
    ) -> Logged<'a, Option<(TurnRecord, LoggedEvent)>> {
        Box::pin(Self::finish(self, request_id, state, fields))
    }

    fn cancel<'a>(
        &'a self,
        request_id: &'a str,
        fields: &'a Map<String, Value>,
    ) -> Logged<'a, Option<(TurnRecord, Option<LoggedEvent>)>> {
        Box::pin(Self::cancel(self, request_id, fields))
    }

    fn withdraw_steered<'a>(
        &'a self,
        request_id: &'a str,
        digest: &'a str,
    ) -> Logged<'a, Option<(TurnRecord, LoggedEvent)>> {
        Box::pin(Self::withdraw_steered(self, request_id, digest))
    }
}

fn tool_input(input: &ToolInput) -> Result<Box<RawValue>, StoreError> {
    match input {
        ToolInput::Function(input) => Ok(input.clone()),
        ToolInput::Freeform(input) => raw(input),
    }
}

impl EffectGate for OneLog {
    fn effect(&self, tool: &str) -> EffectClass {
        self.effect_of(tool)
    }

    fn before<'a>(&'a self, call: EffectCall<'a>) -> HostFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let input = tool_input(call.input).map_err(|error| error.to_string())?;
            let mut lease = self.log.lease().await;
            lease
                .push(Record::Call {
                    call_id: call.call_id.to_owned(),
                    effect: call.effect,
                    input,
                    tool: call.tool.to_owned(),
                    turn: call.turn_id.map(str::to_owned),
                })
                .map_err(|error| error.to_string())?;
            lease
                .commit(Point::Call)
                .await
                .map(drop)
                .map_err(|error| error.to_string())
        })
    }

    fn after<'a>(&'a self, call_id: &'a str, output: &'a ToolOutput) -> HostFuture<'a, ()> {
        Box::pin(async move {
            let Ok(output) = raw(&json!({"success": output.success})) else {
                return;
            };
            if let Err(error) = self.log.lease().await.push(Record::Result {
                call_id: call_id.to_owned(),
                output,
            }) {
                nanocode_trace::warn!(%error, call_id, "turn log could not record a tool result");
            }
        })
    }
}

impl CellJournalSink for OneLogJournals {
    fn save(&self, mut update: CellJournalUpdate) -> HostFuture<'static, Result<(), String>> {
        let log = Arc::clone(&self.0);
        Box::pin(async move {
            let external = update
                .calls
                .iter()
                .filter(|call| {
                    matches!(call.state, JournaledCallState::Running)
                        && log.effect_of(&call.name).needs_row()
                })
                .map(|call| {
                    Ok(Record::Call {
                        call_id: call.call_id.clone(),
                        effect: EffectClass::External,
                        input: raw(&call.input)?,
                        tool: call.name.clone(),
                        turn: None,
                    })
                })
                .collect::<Result<Vec<_>, StoreError>>()
                .map_err(|error| error.to_string())?;
            let mut lease = log.log.lease().await;
            let fresh = update.header.is_some();
            if let Some(header) = update.header.as_mut() {
                let catalog =
                    serde_json::to_string(&header.tools).map_err(|error| error.to_string())?;
                let digest = nanocode_digest::blake3(catalog.as_bytes())
                    .to_hex()
                    .to_string();
                let key = stored_key(TOOLS_MARK, &digest);
                if lease.fold().stored(&key).is_none() {
                    lease
                        .push(Record::Stored {
                            key,
                            value: catalog,
                        })
                        .map_err(|error| error.to_string())?;
                }
                header.tools = [json!({ (TOOLS_MARK): digest })].into();
            }
            lease
                .push(Record::Cell {
                    cell: update.cell_id,
                    fresh,
                    session: update.session_id.clone(),
                    update: raw(&update).map_err(|error| error.to_string())?,
                })
                .map_err(|error| error.to_string())?;
            let cell = (update.session_id.clone(), update.cell_id);
            if update.finished {
                log.live.update(|live| live.remove(&cell));
            } else if fresh {
                log.live.update(|live| live.insert(cell.clone()));
            }
            let seq = lease.fold().seq();
            let timed = update
                .entries
                .iter()
                .any(|entry| matches!(entry, JournalEntry::Timeout { .. }));
            let pending = log.unsaved.read(|unsaved| unsaved.get(&cell) == Some(&seq));
            let started = update
                .calls
                .iter()
                .filter(|call| {
                    matches!(call.state, JournaledCallState::Running)
                        && log.effect_of(&call.name).needs_row()
                })
                .map(|call| call.id)
                .collect::<Vec<_>>();
            let results = update
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    JournalEntry::Result { id, .. } => Some(*id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let answered = log.dispatched.update(|dispatched| {
                let calls = dispatched.entry(cell.clone()).or_default();
                calls.extend(started.iter().copied());
                let before = calls.len();
                calls.retain(|id| !results.contains(id));
                let answered = calls.len() < before;
                if calls.is_empty() || update.finished {
                    dispatched.remove(&cell);
                }
                answered
            });
            let owed = log.answered.read(|owed| owed.get(&cell) == Some(&seq));
            if external.is_empty()
                && !owed
                && !(answered && (timed || update.finished))
                && !(timed && (fresh || pending))
            {
                log.unsaved.update(|unsaved| {
                    if update.finished {
                        unsaved.remove(&cell);
                    } else if fresh {
                        unsaved.insert(cell.clone(), seq);
                    }
                });
                log.answered.update(|owed| {
                    if update.finished {
                        owed.remove(&cell);
                    } else if answered {
                        owed.insert(cell.clone(), seq);
                    }
                });
                return Ok(());
            }
            log.unsaved.update(|unsaved| unsaved.remove(&cell));
            log.answered.update(|owed| owed.remove(&cell));
            for call in external {
                lease.push(call).map_err(|error| error.to_string())?;
            }
            lease
                .commit(Point::Call)
                .await
                .map(drop)
                .map_err(|error| error.to_string())
        })
    }

    fn forget(&self, session_id: &str, cell_id: u64) -> HostFuture<'static, ()> {
        let log = Arc::clone(&self.0);
        let session = session_id.to_owned();
        log.unsaved
            .update(|unsaved| unsaved.remove(&(session.clone(), cell_id)));
        log.answered
            .update(|owed| owed.remove(&(session.clone(), cell_id)));
        log.dispatched
            .update(|dispatched| dispatched.remove(&(session.clone(), cell_id)));
        let unfinished = log
            .live
            .update(|live| live.remove(&(session.clone(), cell_id)));
        Box::pin(async move {
            let mut lease = log.log.lease().await;
            if let Err(error) = lease.push(Record::Forget {
                cell: cell_id,
                session,
            }) {
                nanocode_trace::warn!(%error, "turn log could not forget a cell journal");
                return;
            }
            if unfinished && let Err(error) = lease.commit(Point::Call).await {
                nanocode_trace::warn!(%error, "turn log could not commit a forgotten cell journal");
            }
        })
    }

    fn load(&self, session_id: &str) -> HostFuture<'static, Result<Vec<CellJournal>, String>> {
        let log = Arc::clone(&self.0);
        let session = session_id.to_owned();
        Box::pin(async move {
            Ok(log
                .cells(Some(&session))
                .await
                .map_err(|error| error.to_string())?
                .remove(&session)
                .unwrap_or_default())
        })
    }

    fn save_stored(
        &self,
        session_id: &str,
        stored: String,
    ) -> HostFuture<'static, Result<(), String>> {
        let log = Arc::clone(&self.0);
        let key = stored_key(STORED_MARK, session_id);
        Box::pin(async move {
            log.log
                .lease()
                .await
                .push(Record::Stored { key, value: stored })
                .map_err(|error| error.to_string())
        })
    }

    fn load_stored(&self, session_id: &str) -> HostFuture<'static, Result<Option<String>, String>> {
        let log = Arc::clone(&self.0);
        let key = stored_key(STORED_MARK, session_id);
        Box::pin(async move { Ok(log.read(|fold| fold.stored(&key).map(str::to_owned)).await) })
    }
}

impl OneLogStore {
    fn replaced(
        &self,
        lease: &mut nanocode_turnlog::Lease<'_>,
        state_id: &str,
        owner: &OwnerToken,
        expected_revision: u64,
        payload: &str,
        records: &[StoreRecord],
    ) -> Result<u64, StoreError> {
        if self
            .0
            .owners
            .read(|owners| owners.get(state_id) != Some(owner))
        {
            return Err(StoreError::Fenced);
        }
        let actual = lease
            .fold()
            .checkpoint(state_id)
            .map_or(0, |checkpoint| checkpoint.revision);
        if actual != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                actual,
            });
        }
        let revision = actual + 1;
        for record in records {
            let key = stored_key(state_id, &record.key);
            if lease.fold().stored(&key) != Some(record.value.as_str()) {
                lease.push(Record::Stored {
                    key,
                    value: record.value.clone(),
                })?;
            }
        }
        lease.push(Record::Checkpoint {
            payload: payload.to_owned(),
            revision,
            state: state_id.to_owned(),
        })?;
        Ok(revision)
    }

    fn stored_state(fold: &Fold, state_id: &str) -> StoredState {
        fold.checkpoint(state_id)
            .map(|checkpoint| StoredState {
                revision: checkpoint.revision,
                payload: Some(checkpoint.payload.clone()),
            })
            .unwrap_or_default()
    }
}

impl StateStore for OneLogStore {
    fn read_record<'a>(
        &'a mut self,
        state_id: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Result<Option<String>, StoreError>> {
        Box::pin(async move {
            Ok(self
                .0
                .log
                .read(|fold| fold.stored(&stored_key(state_id, key)).map(str::to_owned))
                .await)
        })
    }

    fn read_records<'a>(
        &'a mut self,
        state_id: &'a str,
        keys: &'a [String],
    ) -> StoreFuture<'a, Result<Vec<Option<String>>, StoreError>> {
        Box::pin(async move {
            Ok(self
                .0
                .log
                .read(|fold| {
                    keys.iter()
                        .map(|key| fold.stored(&stored_key(state_id, key)).map(str::to_owned))
                        .collect()
                })
                .await)
        })
    }

    fn peek<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<StoredState, StoreError>> {
        Box::pin(async move {
            Ok(self
                .0
                .log
                .read(|fold| Self::stored_state(fold, state_id))
                .await)
        })
    }

    fn list_states<'a>(
        &'a mut self,
        limit: usize,
    ) -> StoreFuture<'a, Result<Vec<String>, StoreError>> {
        Box::pin(async move {
            Ok(self
                .0
                .log
                .read(|fold| {
                    fold.checkpoints()
                        .take(limit)
                        .map(|(state, _)| state.to_owned())
                        .collect()
                })
                .await)
        })
    }

    fn owner<'a>(
        &'a mut self,
        state_id: &'a str,
    ) -> StoreFuture<'a, Result<Option<OwnerToken>, StoreError>> {
        let owner = self.0.owners.read(|owners| owners.get(state_id).cloned());
        Box::pin(async move { Ok(owner) })
    }

    fn acquire<'a>(
        &'a mut self,
        state_id: &'a str,
        owner_id: OwnerId,
    ) -> StoreFuture<'a, Result<OwnedState, StoreError>> {
        Box::pin(async move {
            let lease = self.0.log.lease().await;
            let owner = self.0.owners.update(|owners| {
                let fence = owners.get(state_id).map_or(0, OwnerToken::fence) + 1;
                let token = OwnerToken::new(owner_id.clone(), fence);
                owners.insert(state_id.to_owned(), token.clone());
                token
            });
            Ok(OwnedState {
                owner,
                state: Self::stored_state(lease.fold(), state_id),
            })
        })
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
            let mut lease = self.0.log.lease().await;
            let revision = self.replaced(
                &mut lease,
                state_id,
                owner,
                expected_revision,
                payload,
                records,
            )?;
            if *self.0.durable_settles.get() {
                lease.commit(Point::Progress).await?;
            }
            Ok(revision)
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
            let mut lease = self.0.log.lease().await;
            let revision = self.replaced(
                &mut lease,
                state_id,
                owner,
                expected_revision,
                commit.payload,
                commit.records,
            )?;
            if commit.settle == Settle::Durable
                && (lease.fold().recovering() || *self.0.durable_settles.get())
            {
                lease.commit(Point::Progress).await?;
            }
            Ok(Committed {
                revision,
                first_event: None,
            })
        })
    }

    fn put_records<'a>(
        &'a mut self,
        state_id: &'a str,
        records: &'a [StoreRecord],
    ) -> StoreFuture<'a, Result<bool, StoreError>> {
        Box::pin(async move {
            let mut lease = self.0.log.lease().await;
            for record in records {
                let key = stored_key(state_id, &record.key);
                if lease.fold().stored(&key) != Some(record.value.as_str()) {
                    lease.push(Record::Stored {
                        key,
                        value: record.value.clone(),
                    })?;
                }
            }
            Ok(true)
        })
    }
}
