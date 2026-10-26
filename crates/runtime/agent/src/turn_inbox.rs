use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use nanocode_subagents::{AgentUpdate, ScopedAgentUpdate};
use nanocode_sync::{Atom, Owned};
use nanocode_tool_host::embedded::HostFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    Agent, AgentRuntime, DurableOutcome, EventSink, NanocodeError, PromptRequest, SessionEvent,
    SessionEvents, SteerReceiptStatus,
    durability::{SessionRecord, SessionStore, Sql, StoreError},
    events::{
        AGENT_EVENT_PROTOCOL_VERSION, AcceptedInput, AgentEvent, AgentEventKind, AgentEventTiming,
    },
    input::{Prompt, PromptInput},
    session_log::{LogEntry, LoggedEvent, SessionLog, Started, TurnRecord, TurnState, now},
    stored_event::Stored,
};

pub trait InboxHost: Send + Sync + 'static {
    fn publish(&self, events: Vec<LoggedEvent>) -> HostFuture<'_, Result<(), String>>;

    fn live_deltas(&self) -> bool {
        false
    }

    fn publish_live(&self, _events: Vec<LoggedEvent>) -> HostFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn annotate(&self) -> Map<String, Value> {
        Map::new()
    }

    fn finishing(&self, _request_id: &str) -> HostFuture<'_, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }

    fn place(&self, _session_id: &str, _hand: Option<&str>) {}

    fn idle(&self) -> bool {
        true
    }
}

#[derive(Debug)]
pub enum InboxError {
    Store(StoreError),
    Agent(NanocodeError),
    Events(String),
}

impl fmt::Display for InboxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Agent(error) => error.fmt(formatter),
            Self::Events(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for InboxError {}

impl From<StoreError> for InboxError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<NanocodeError> for InboxError {
    fn from(error: NanocodeError) -> Self {
        Self::Agent(error)
    }
}

#[derive(Clone, Debug)]
pub enum Driven {
    Idle,
    Busy,
    Finished(TurnRecord),
    Resumed(String),
    Waiting(String),
    Interrupted(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MovedQueued {
    Moved,
    NotQueued,
    NotRunning,
    Conflict,
    Unknown,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Run {
    Inbox,
    Resumed,
}

#[derive(Clone, Default)]
struct Request {
    running: Option<Run>,
    cancelled: bool,
}

#[derive(Clone, Default)]
struct Requests {
    requests: HashMap<String, Request>,
    turns: HashMap<String, String>,
}

impl Requests {
    fn cancelled(&self, id: &str) -> bool {
        self.requests
            .get(id)
            .is_some_and(|request| request.cancelled)
    }

    fn idle(&self) -> bool {
        !self
            .requests
            .values()
            .any(|request| request.running.is_some())
    }

    fn tidy(&mut self, id: &str) {
        if self
            .requests
            .get(id)
            .is_some_and(|request| request.running.is_none() && !request.cancelled)
        {
            self.requests.remove(id);
        }
    }

    fn finished(&mut self, id: &str) {
        if let Some(request) = self.requests.get_mut(id) {
            request.cancelled = false;
        }
        self.tidy(id);
    }
}

struct Shared<H: ?Sized> {
    log: Arc<SessionLog<H>>,
    host: Arc<dyn InboxHost>,
    state: Atom<Requests>,
    session_id: String,
    streams: Owned<Vec<Stream>>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Streamed {
    Assistant,
    ToolInput,
}

struct Stream {
    kind: Streamed,
    key: String,
    event: AgentEvent,
    timing: AgentEventTiming,
    received_ns: u64,
    text: String,
    offset: usize,
}

enum Role {
    Delta(Streamed, String, String),
    Final(Streamed, String),
    Boundary { ends_run: bool },
    Other,
}

#[derive(Default, Deserialize)]
struct StreamFields {
    #[serde(default)]
    item_id: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

fn role(event: &SessionEvent) -> Role {
    let SessionEvent::Agent { event, .. } = event else {
        return Role::Other;
    };
    let streamed = match event.kind {
        AgentEventKind::AssistantDelta | AgentEventKind::AssistantMessage => Streamed::Assistant,
        AgentEventKind::ToolInputDelta | AgentEventKind::ToolCall => Streamed::ToolInput,
        AgentEventKind::RunCompleted | AgentEventKind::RunFailed => {
            return Role::Boundary { ends_run: true };
        }
        AgentEventKind::RunError
        | AgentEventKind::RunSteered
        | AgentEventKind::ModelCallFailed
        | AgentEventKind::ModelAttemptFailed
        | AgentEventKind::ModelAttemptRetrying => return Role::Boundary { ends_run: false },
        _ => return Role::Other,
    };
    let fields: StreamFields = serde_json::from_str(event.payload.get()).unwrap_or_default();
    let key = match streamed {
        Streamed::Assistant => fields.item_id,
        Streamed::ToolInput => fields.call_id,
    };
    let Some(key) = key else {
        return Role::Other;
    };
    match (event.kind, fields.text) {
        (AgentEventKind::AssistantDelta | AgentEventKind::ToolInputDelta, Some(text)) => {
            Role::Delta(streamed, key, text)
        }
        (AgentEventKind::AssistantDelta | AgentEventKind::ToolInputDelta, None) => Role::Other,
        _ => Role::Final(streamed, key),
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn rewritten(
    event: &AgentEvent,
    text: Option<&str>,
    offset: usize,
) -> Result<AgentEvent, serde_json::Error> {
    let mut fields: Map<String, Value> = serde_json::from_str(event.payload.get())?;
    if let Some(text) = text {
        fields.insert("text".into(), Value::String(text.to_owned()));
    }
    fields.insert("offset".into(), Value::from(offset));
    let mut event = event.clone();
    event.payload = Arc::from(serde_json::value::to_raw_value(&fields)?);
    Ok(event)
}

impl Stream {
    fn merged(&self) -> Result<SessionEvent, serde_json::Error> {
        Ok(SessionEvent::Agent {
            event: rewritten(&self.event, Some(&self.text), self.offset)?,
            timing: self.timing,
            received_ns: self.received_ns,
        })
    }
}

fn stream(
    streams: &mut Vec<Stream>,
    kind: Streamed,
    key: String,
    text: &str,
    event: SessionEvent,
) -> Result<SessionEvent, serde_json::Error> {
    let SessionEvent::Agent {
        event,
        timing,
        received_ns,
    } = event
    else {
        return Ok(event);
    };
    let index = match streams
        .iter()
        .position(|stream| stream.kind == kind && stream.key == key)
    {
        Some(index) => index,
        None => {
            streams.push(Stream {
                kind,
                key,
                event: event.clone(),
                timing,
                received_ns,
                text: String::new(),
                offset: 0,
            });
            streams.len() - 1
        }
    };
    let entry = &mut streams[index];
    let offset = entry.offset + utf16_len(&entry.text);
    entry.text.push_str(text);
    entry.event = event.clone();
    entry.timing = timing;
    entry.received_ns = received_ns;
    Ok(SessionEvent::Agent {
        event: rewritten(&event, None, offset)?,
        timing,
        received_ns,
    })
}

fn flush(
    streams: &mut Vec<Stream>,
    ends_run: bool,
) -> Result<Vec<SessionEvent>, serde_json::Error> {
    let mut merged = Vec::new();
    for entry in streams.iter_mut().filter(|entry| !entry.text.is_empty()) {
        merged.push(entry.merged()?);
        entry.offset += utf16_len(&entry.text);
        entry.text.clear();
    }
    if ends_run {
        streams.clear();
    }
    Ok(merged)
}

#[derive(Default, Deserialize)]
struct Identifiers {
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Serialize)]
struct Envelope<'a> {
    #[serde(flatten)]
    event: &'a SessionEvent,
    #[serde(flatten)]
    annotation: &'a Map<String, Value>,
}

pub(crate) fn accepted_bytes(
    request_id: &str,
    input: &PromptInput,
) -> Result<usize, serde_json::Error> {
    let generated = nanocode_platform::uuid_v7().to_string();
    let turn = if request_id.len() > generated.len() {
        request_id
    } else {
        &generated
    };
    let empty = PromptInput::Text(String::new());
    let payload = serde_json::value::to_raw_value(&AcceptedInput {
        session_id: generated.clone(),
        turn_id: turn.to_owned(),
        item_id: format!("{turn}:prompt"),
        kind: "prompt".into(),
        request_id: Some(request_id.to_owned()),
        input: empty.clone(),
    })?;
    let event = SessionEvent::Agent {
        event: AgentEvent {
            protocol_version: AGENT_EVENT_PROTOCOL_VERSION,
            request_id: generated.as_str().into(),
            seq: u64::MAX,
            kind: AgentEventKind::InputAccepted,
            payload: payload.into(),
        },
        timing: AgentEventTiming {
            emitted_ns: 10_u64.pow(u64::MAX.ilog10()),
            source_received_ns: Some(u64::MAX),
        },
        received_ns: u64::MAX,
    };
    let stored = serde_json::value::to_raw_value(&Stored {
        event: &event,
        compact: None,
    })?;
    Ok(stored.get().len() - serde_json::to_vec(&empty)?.len() + serde_json::to_vec(input)?.len())
}

impl<H: ?Sized> Shared<H> {
    fn attribute(&self, event: &SessionEvent) -> Option<String> {
        let (event, agent) = match event {
            SessionEvent::Agent { event, .. } => (event, true),
            SessionEvent::Subagent(ScopedAgentUpdate {
                update: AgentUpdate::Event { event, .. },
                ..
            }) => (event, false),
            SessionEvent::Subagent(_) => return None,
        };
        let ids: Identifiers = serde_json::from_str(event.payload.get()).unwrap_or_default();
        match (ids.turn_id, ids.request_id) {
            (Some(turn), _) if !agent => Some(turn),
            (Some(turn), Some(accepted))
                if self.state.read(|state| {
                    state
                        .requests
                        .get(&accepted)
                        .is_some_and(|request| request.running == Some(Run::Inbox))
                }) =>
            {
                self.state.update(|state| {
                    state.turns.insert(turn.clone(), accepted.clone());
                });
                Some(accepted)
            }
            (Some(turn), _) => Some(
                self.state
                    .read(|state| state.turns.get(&turn).cloned())
                    .unwrap_or(turn),
            ),
            (None, accepted) => accepted,
        }
    }
}

struct LogSink<H: ?Sized>(Arc<Shared<H>>);

impl<H: Sql + Send + Sync + ?Sized + 'static> LogSink<H> {
    async fn persist(&self, events: Vec<SessionEvent>) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        let annotation = self.0.host.annotate();
        let unannotated = Map::new();
        let compact = self.0.log.compact.then_some(&*self.0.session_id);
        let (entries, live): (Vec<_>, Vec<_>) = events
            .iter()
            .map(|event| {
                let request_id = self.0.attribute(event);
                let mut body = serde_json::value::to_raw_value(&Envelope {
                    event,
                    annotation: &annotation,
                })?;
                if !annotation.is_empty()
                    && !self.0.log.event_fits(request_id.as_deref(), body.get())
                {
                    body = serde_json::value::to_raw_value(&Envelope {
                        event,
                        annotation: &unannotated,
                    })?;
                }
                let stored = serde_json::value::to_raw_value(&Stored { event, compact })?;
                Ok((
                    LogEntry {
                        request_id,
                        event: stored,
                    },
                    body,
                ))
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()
            .map_err(|error| format!("encode agent event: {error}"))?
            .into_iter()
            .unzip();
        let logged = self
            .0
            .log
            .append(&entries)
            .await
            .map_err(|error| error.to_string())?;
        let published = logged
            .into_iter()
            .zip(&entries)
            .zip(live)
            .map(|((mut logged, entry), live)| {
                if logged.event.get() == entry.event.get() {
                    logged.event = live;
                }
                logged
            })
            .collect();
        self.0.host.publish(published).await
    }

    async fn publish_live(&self, events: Vec<SessionEvent>) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        let annotation = self.0.host.annotate();
        let created_at = now();
        let live = events
            .iter()
            .map(|event| {
                Ok(LoggedEvent {
                    seq: 0,
                    request_id: self.0.attribute(event),
                    created_at,
                    event: serde_json::value::to_raw_value(&Envelope {
                        event,
                        annotation: &annotation,
                    })?,
                })
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()
            .map_err(|error| format!("encode agent event: {error}"))?;
        self.0.host.publish_live(live).await
    }

    fn route(
        streams: &mut Vec<Stream>,
        events: Vec<SessionEvent>,
    ) -> Result<Vec<(bool, Vec<SessionEvent>)>, String> {
        let encode = |error: serde_json::Error| format!("encode agent event: {error}");
        let mut segments: Vec<(bool, Vec<SessionEvent>)> = Vec::new();
        let mut push = |live: bool, event: SessionEvent| match segments.last_mut() {
            Some((kind, events)) if *kind == live => events.push(event),
            _ => segments.push((live, vec![event])),
        };
        for event in events {
            match role(&event) {
                Role::Delta(kind, key, text) => {
                    push(
                        true,
                        stream(streams, kind, key, &text, event).map_err(encode)?,
                    );
                }
                Role::Final(kind, key) => {
                    streams.retain(|entry| entry.kind != kind || entry.key != key);
                    push(false, event);
                }
                Role::Boundary { ends_run } => {
                    for merged in flush(streams, ends_run).map_err(encode)? {
                        push(false, merged);
                    }
                    push(false, event);
                }
                Role::Other => push(false, event),
            }
        }
        Ok(segments)
    }
}

impl<H: Sql + Send + Sync + ?Sized + 'static> EventSink for LogSink<H> {
    fn deliver(&self, events: Vec<SessionEvent>) -> HostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if !self.0.host.live_deltas() {
                return self.persist(events).await;
            }
            let segments = Self::route(&mut *self.0.streams.acquire().await, events)?;
            for (live, events) in segments {
                if live {
                    self.publish_live(events).await?;
                } else {
                    self.persist(events).await?;
                }
            }
            Ok(())
        })
    }
}

struct Claim<'a, H: ?Sized>(&'a Shared<H>, &'a str);

impl<H: ?Sized> Drop for Claim<'_, H> {
    fn drop(&mut self) {
        self.0.state.update(|state| {
            if let Some(request) = state.requests.get_mut(self.1) {
                request.running = None;
            }
            state.tidy(self.1);
        });
    }
}

struct Busy<'a>(&'a AtomicBool);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn ignore_inactive(result: Result<(), NanocodeError>) -> Result<(), NanocodeError> {
    match result {
        Err(NanocodeError::TurnNotCancellable) => Ok(()),
        other => other,
    }
}

fn fields(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(fields) => fields,
        _ => Map::new(),
    }
}

pub struct TurnInbox<H: ?Sized> {
    shared: Arc<Shared<H>>,
    runtime: AgentRuntime,
    agent: Agent,
    events: SessionEvents,
    model: String,
    busy: AtomicBool,
}

impl<H: Sql + Send + Sync + ?Sized + 'static> TurnInbox<H> {
    pub async fn open(
        runtime: AgentRuntime,
        store: &SessionStore,
        record: SessionRecord,
        log: Arc<SessionLog<H>>,
        host: Arc<dyn InboxHost>,
        window: Duration,
    ) -> Result<(Self, HostFuture<'static, ()>), InboxError> {
        let session_id = record.session_id.clone();
        let catalog = |error| InboxError::Agent(crate::runtime::catalog_error(error));
        if store.summary(&session_id).await.map_err(catalog)?.is_none() {
            store.session(record).await.map_err(catalog)?;
        }
        let subagents = runtime.take_subagent_updates();
        let stored = store.load(&session_id).await.map_err(catalog)?;
        let (agent, stream) = runtime.open_loaded(store, &stored).await?;
        agent.leave_interrupted_turns_to_host();
        let shared = Arc::new(Shared {
            log,
            host,
            state: Atom::default(),
            session_id,
            streams: Owned::new(Vec::new()),
        });
        let (events, pump) =
            SessionEvents::start(stream, subagents, window, LogSink(Arc::clone(&shared)));
        Ok((
            Self {
                shared,
                runtime,
                agent,
                events,
                model: stored.summary.record.model.as_str().to_owned(),
                busy: AtomicBool::new(false),
            },
            pump,
        ))
    }

    #[must_use]
    pub const fn agent(&self) -> &Agent {
        &self.agent
    }

    #[must_use]
    pub fn log(&self) -> &Arc<SessionLog<H>> {
        &self.shared.log
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        self.agent.session_id()
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn deliver_completions(&self) {
        if let Some(control) = self.runtime.subagents() {
            control
                .deliver_completions(&self.agent, self.agent.session_id())
                .await;
        }
    }

    pub async fn drive(&self, next: Option<&str>) -> Result<Driven, InboxError> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return Ok(Driven::Busy);
        }
        let _busy = Busy(&self.busy);
        let root = self.session_id();
        if let Some(control) = self.runtime.subagents() {
            control.await_restored(&self.agent, root).await;
        }
        if let Some(first) = self.runtime.pending_turns(root).await?.into_iter().next() {
            return match self.shared.log.turn(&first).await? {
                Some(record) if !record.state.is_terminal() => self.run(&first, true).await,
                record => {
                    let abandoned = record
                        .as_ref()
                        .is_some_and(|record| record.state != TurnState::Completed);
                    self.resume(first, record.and_then(|record| record.hand), abandoned)
                        .await
                }
            };
        }
        let next = match next {
            Some(next) => Some(next.to_owned()),
            None => self
                .shared
                .log
                .pending()
                .await?
                .into_iter()
                .next()
                .map(|record| record.request_id),
        };
        match next {
            Some(next) => self.run(&next, false).await,
            None => Ok(Driven::Idle),
        }
    }

    pub async fn cancel(
        &self,
        request_id: &str,
        fields: &Map<String, Value>,
    ) -> Result<Option<(TurnRecord, bool)>, InboxError> {
        let Some((record, event)) = self.shared.log.cancel(request_id, fields).await? else {
            return Ok(None);
        };
        let dequeued = event.is_some();
        if let Some(event) = event {
            self.publish(event).await?;
        }
        if record.state == TurnState::Running {
            self.shared.state.update(|state| {
                state
                    .requests
                    .entry(request_id.to_owned())
                    .or_default()
                    .cancelled = true;
            });
            let live = self
                .agent
                .turn_controls()
                .into_iter()
                .find(|(id, _)| id == request_id);
            if let Some((_, control)) = live {
                ignore_inactive(control.cancel().await)?;
            }
        }
        Ok(Some((record, dequeued)))
    }

    pub async fn steer(
        &self,
        target: Option<&str>,
        id: &str,
        prompt: Prompt,
    ) -> Result<bool, InboxError> {
        for (request_id, control) in self.agent.turn_controls() {
            if target.is_some_and(|target| target != request_id)
                || self.shared.state.read(|state| state.cancelled(&request_id))
            {
                continue;
            }
            match control.steer_with_id(id.to_owned(), prompt.clone()).await {
                Ok(()) => return Ok(true),
                Err(NanocodeError::TurnNotSteerable | NanocodeError::TurnStopped) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(false)
    }

    pub async fn move_queued(
        &self,
        queued_id: &str,
        target: Option<&str>,
    ) -> Result<MovedQueued, InboxError> {
        let Some(queued) = self.shared.log.turn(queued_id).await? else {
            return Ok(MovedQueued::NotQueued);
        };
        let Some(prompt) = queued
            .prompt
            .clone()
            .filter(|_| queued.state == TurnState::Queued)
        else {
            let steered = queued.report()?.get("steered") == Some(&Value::Bool(true));
            return Ok(if steered {
                MovedQueued::Moved
            } else {
                MovedQueued::NotQueued
            });
        };
        match self
            .agent
            .steer_receipt(queued_id.to_owned(), prompt.clone())
            .await?
        {
            SteerReceiptStatus::Accepted => {}
            SteerReceiptStatus::Absent => {
                if !self.steer(target, queued_id, prompt).await? {
                    return Ok(MovedQueued::NotRunning);
                }
            }
            SteerReceiptStatus::Different | SteerReceiptStatus::Withdrawn => {
                return Ok(MovedQueued::Conflict);
            }
            SteerReceiptStatus::Unknown => return Ok(MovedQueued::Unknown),
        }
        self.withdraw_steered(&queued).await?;
        Ok(MovedQueued::Moved)
    }

    pub async fn settle(&self) -> Result<bool, InboxError> {
        self.events.drained().await.map_err(InboxError::Events)?;
        if !self.idle() {
            return Ok(false);
        }
        Ok(self.runtime.settle(self.session_id()).await? && self.idle())
    }

    pub async fn shutdown(&self) -> Result<(), InboxError> {
        let shutdown = self.runtime.shutdown().await;
        let delivered = self.events.drained().await;
        shutdown?;
        delivered.map_err(InboxError::Events)
    }

    fn idle(&self) -> bool {
        self.shared.state.read(Requests::idle) && self.shared.host.idle()
    }

    async fn publish(&self, event: LoggedEvent) -> Result<(), InboxError> {
        self.shared
            .host
            .publish(vec![event])
            .await
            .map_err(InboxError::Events)
    }

    async fn withdraw_steered(
        &self,
        queued: &TurnRecord,
    ) -> Result<Option<TurnRecord>, InboxError> {
        let Some((record, event)) = self
            .shared
            .log
            .withdraw_steered(&queued.request_id, &queued.digest)
            .await?
        else {
            return Ok(None);
        };
        self.publish(event).await?;
        Ok(Some(record))
    }

    async fn run(&self, id: &str, accepted: bool) -> Result<Driven, InboxError> {
        if !accepted
            && let Some(queued) = self.shared.log.turn(id).await?
            && queued.state == TurnState::Queued
            && let Some(prompt) = queued.prompt.clone()
            && self.agent.steer_receipt(id.to_owned(), prompt).await?
                == SteerReceiptStatus::Accepted
            && let Some(record) = self.withdraw_steered(&queued).await?
        {
            return Ok(Driven::Finished(record));
        }
        let Some(Started {
            previous,
            record,
            event,
        }) = self.shared.log.start(id).await?
        else {
            return Ok(Driven::Idle);
        };
        self.publish(event).await?;
        self.shared
            .host
            .place(self.session_id(), record.hand.as_deref());
        let stored = if accepted || previous == TurnState::Running {
            self.runtime.durable_prompt(&self.agent, id).await?
        } else {
            None
        };
        let cancel = record.cancel_requested;
        let Some(prompt) = stored.or(record.prompt) else {
            return if cancel {
                self.finish(
                    id,
                    TurnState::Cancelled,
                    fields(serde_json::json!({"cancelled": true})),
                )
                .await
            } else {
                self.finish(
                    id,
                    TurnState::Failed,
                    fields(serde_json::json!({"error": "the turn's prompt is no longer stored"})),
                )
                .await
            };
        };
        match self.execute(id, prompt, cancel, Run::Inbox).await {
            DurableOutcome::Completed(message) => {
                let ok = serde_json::json!({"ok": {
                    "session_id": self.session_id(),
                    "model": self.model,
                    "message": message,
                }});
                self.finish(id, TurnState::Completed, fields(ok)).await
            }
            DurableOutcome::Failed(error) => {
                self.finish(
                    id,
                    TurnState::Failed,
                    fields(serde_json::json!({"error": error})),
                )
                .await
            }
            DurableOutcome::Cancelled => {
                self.finish(
                    id,
                    TurnState::Cancelled,
                    fields(serde_json::json!({"cancelled": true})),
                )
                .await
            }
            DurableOutcome::Waiting => Ok(Driven::Waiting(id.to_owned())),
            DurableOutcome::Interrupted => Ok(Driven::Interrupted(id.to_owned())),
        }
    }

    async fn resume(
        &self,
        id: String,
        hand: Option<String>,
        abandoned: bool,
    ) -> Result<Driven, InboxError> {
        let Some(prompt) = self.runtime.durable_prompt(&self.agent, &id).await? else {
            return Ok(Driven::Resumed(id));
        };
        self.shared.host.place(self.session_id(), hand.as_deref());
        let cancel = abandoned || self.shared.state.read(|state| state.cancelled(&id));
        let outcome = self.execute(&id, prompt, cancel, Run::Resumed).await;
        self.shared.state.update(|state| state.finished(&id));
        Ok(match outcome {
            DurableOutcome::Waiting => Driven::Waiting(id),
            DurableOutcome::Interrupted => Driven::Interrupted(id),
            _ => Driven::Resumed(id),
        })
    }

    async fn finish(
        &self,
        id: &str,
        state: TurnState,
        mut outcome: Map<String, Value>,
    ) -> Result<Driven, InboxError> {
        outcome.extend(self.shared.host.finishing(id).await);
        let finished = self.shared.log.finish(id, state, &outcome).await?;
        self.shared.state.update(|state| state.finished(id));
        let Some((record, event)) = finished else {
            return Ok(match self.shared.log.turn(id).await? {
                Some(record) => Driven::Finished(record),
                None => Driven::Idle,
            });
        };
        self.publish(event).await?;
        Ok(Driven::Finished(record))
    }

    async fn execute(&self, id: &str, prompt: Prompt, cancel: bool, run: Run) -> DurableOutcome {
        let claimed = self.shared.state.update(|state| {
            let request = state.requests.entry(id.to_owned()).or_default();
            if request.running.is_some() {
                return false;
            }
            request.running = Some(run);
            request.cancelled |= cancel;
            true
        });
        if !claimed {
            return DurableOutcome::Waiting;
        }
        let _claim = Claim(&self.shared, id);
        let outcome = self.prompt(id, prompt, cancel).await;
        if self.events.drained().await.is_err() {
            return DurableOutcome::Interrupted;
        }
        self.shared.state.update(|state| {
            state.turns.retain(|_, request| request != id);
        });
        self.runtime
            .durable_outcome(self.session_id(), id, outcome)
            .await
    }

    async fn prompt(
        &self,
        id: &str,
        prompt: Prompt,
        cancel: bool,
    ) -> Result<String, NanocodeError> {
        let cancelled = || self.shared.state.read(|state| state.cancelled(id));
        let mut request = PromptRequest::new(prompt).request_id(id);
        if cancel || cancelled() {
            request = request.cancel_on_admission();
        }
        let turn = self.agent.prompt(request).await?;
        if cancelled() {
            ignore_inactive(turn.cancel().await)?;
        }
        let message = turn.result().await?.into_final_message();
        self.agent.flush().await?;
        Ok(message)
    }
}
