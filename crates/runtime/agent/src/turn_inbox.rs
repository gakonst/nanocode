use std::{
    collections::HashMap,
    fmt,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures_util::future::{Either, select};
use nanocode_subagents::{AgentId, AgentUpdate, ScopedAgentUpdate};
use nanocode_sync::{Atom, Owned};
use nanocode_tool_host::embedded::HostFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    Agent, AgentEvents, AgentRuntime, DurableOutcome, EventSink, NanocodeError, PromptRequest,
    SessionEvent, SessionEvents, SteerReceiptStatus, SubagentUpdates, TurnControl, TurnResult,
    durability::{SessionRecord, SessionStore, StoreError},
    events::{
        AGENT_EVENT_PROTOCOL_VERSION, AcceptedInput, AgentEvent, AgentEventKind, AgentEventTiming,
    },
    input::{Prompt, PromptInput},
    session_log::{LoggedEvent, Started, TurnRecord, TurnState, now},
    stored_event::Stored,
    turn_log::{EventLog, Recording, TurnLog},
};

pub trait InboxHost: Send + Sync + 'static {
    fn publish(&self, events: Vec<LoggedEvent>) -> HostFuture<'_, Result<(), String>>;

    fn live_deltas(&self) -> bool {
        false
    }

    fn live_progress(&self) -> bool {
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

    fn coalesce_deltas(&self) -> bool {
        true
    }

    fn inputs<'a>(&'a self, _request_id: &'a str) -> HostFuture<'a, Result<Vec<String>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn input<'a>(
        &'a self,
        _request_id: &'a str,
        _id: &'a str,
    ) -> HostFuture<'a, Result<Prompt, String>> {
        Box::pin(async { Err("this host gives a turn no inputs".to_owned()) })
    }

    fn wake<'a>(&'a self, _request_id: &'a str) -> HostFuture<'a, Wake> {
        Box::pin(std::future::pending())
    }

    fn check<'a>(&'a self, _request_id: &'a str, _wake: Wake) -> HostFuture<'a, Option<Halt>> {
        Box::pin(async { None })
    }

    fn passing<'a>(
        &'a self,
        _request_id: &'a str,
        _pass: Option<&'a str>,
    ) -> HostFuture<'a, Option<Prompt>> {
        Box::pin(async { None })
    }

    fn next<'a>(
        &'a self,
        _request_id: &'a str,
        _end: PassEnd,
        _delivery: &'a Delivery,
    ) -> HostFuture<'a, Next> {
        Box::pin(async { Next::Done })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wake {
    Inputs,
    Changed,
    Tick,
}

pub enum Halt {
    Stop(String),
    Leave,
}

pub enum PassEnd {
    Refused(NanocodeError),
    Completed(TurnResult),
    Failed(NanocodeError),
    Stopped {
        reason: String,
        control: TurnControl,
    },
    Left {
        control: TurnControl,
    },
}

pub enum Next {
    Pass {
        request: PromptRequest,
        read: Vec<String>,
    },
    Done,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Delivered {
    Ungiven,
    Accepted,
    Read,
}

impl Delivered {
    const fn next(self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Ungiven, Self::Accepted | Self::Read)
                | (Self::Accepted, Self::Read | Self::Ungiven)
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Delivery(HashMap<String, Delivered>);

impl Delivery {
    fn of(&self, id: &str) -> Delivered {
        self.0.get(id).copied().unwrap_or(Delivered::Ungiven)
    }

    fn mark(&mut self, id: &str, to: Delivered) -> bool {
        let moved = self.of(id).next(to);
        if moved {
            self.0.insert(id.to_owned(), to);
        }
        moved
    }

    fn ended(&mut self) {
        for delivered in self.0.values_mut() {
            if *delivered == Delivered::Accepted {
                *delivered = Delivered::Ungiven;
            }
        }
    }

    #[must_use]
    pub fn is_read(&self, id: &str) -> bool {
        self.of(id) == Delivered::Read
    }

    #[must_use]
    pub fn read(&self) -> Vec<String> {
        self.with(|delivered| delivered == Delivered::Read)
    }

    #[must_use]
    pub fn given(&self) -> Vec<String> {
        self.with(|delivered| delivered != Delivered::Ungiven)
    }

    fn with(&self, keep: impl Fn(Delivered) -> bool) -> Vec<String> {
        self.0
            .iter()
            .filter(|(_, delivered)| keep(**delivered))
            .map(|(id, _)| id.clone())
            .collect()
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

struct Shared<L: ?Sized> {
    log: Arc<L>,
    host: Arc<dyn InboxHost>,
    state: Atom<Requests>,
    session_id: String,
    streams: Owned<Vec<Stream>>,
    delivery: Atom<Delivery>,
    recording: AtomicBool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Streamed {
    Assistant,
    ToolInput,
    Reasoning,
}

type Scope = Option<(String, AgentId)>;

enum Origin {
    Agent {
        timing: AgentEventTiming,
        received_ns: u64,
    },
    Subagent {
        detach: bool,
        root_session_id: String,
        id: AgentId,
        parent: Option<AgentId>,
    },
}

impl Origin {
    fn of(event: SessionEvent) -> Result<(Self, AgentEvent), SessionEvent> {
        match event {
            SessionEvent::Agent {
                event,
                timing,
                received_ns,
            } => Ok((
                Self::Agent {
                    timing,
                    received_ns,
                },
                event,
            )),
            SessionEvent::Subagent(ScopedAgentUpdate {
                detach,
                root_session_id,
                update: AgentUpdate::Event { id, parent, event },
            }) => Ok((
                Self::Subagent {
                    detach,
                    root_session_id,
                    id,
                    parent,
                },
                event,
            )),
            event @ SessionEvent::Subagent(_) => Err(event),
        }
    }

    fn event(&self, event: AgentEvent) -> SessionEvent {
        match self {
            Self::Agent {
                timing,
                received_ns,
            } => SessionEvent::Agent {
                event,
                timing: *timing,
                received_ns: *received_ns,
            },
            Self::Subagent {
                detach,
                root_session_id,
                id,
                parent,
            } => SessionEvent::Subagent(ScopedAgentUpdate {
                detach: *detach,
                root_session_id: root_session_id.clone(),
                update: AgentUpdate::Event {
                    id: *id,
                    parent: *parent,
                    event,
                },
            }),
        }
    }
}

struct Stream {
    scope: Scope,
    kind: Streamed,
    key: String,
    origin: Origin,
    event: AgentEvent,
    text: String,
    offset: usize,
}

enum Role {
    Delta(Scope, Streamed, String, String),
    Final(Scope, Streamed, String),
    Settled(Scope),
    Boundary { scope: Scope, ends_run: bool },
    Progress,
    Other,
}

#[derive(Default, Deserialize)]
struct StreamFields {
    #[serde(default)]
    item_id: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    model_call_index: Option<u64>,
    #[serde(default)]
    text: Option<String>,
}

fn role(event: &SessionEvent) -> Role {
    let (scope, event) = match event {
        SessionEvent::Agent { event, .. } => (None, event),
        SessionEvent::Subagent(ScopedAgentUpdate {
            root_session_id,
            update: AgentUpdate::Event { id, event, .. },
            ..
        }) => (Some((root_session_id.clone(), *id)), event),
        SessionEvent::Subagent(_) => return Role::Other,
    };
    let streamed = match event.kind {
        AgentEventKind::AssistantDelta | AgentEventKind::AssistantMessage => Streamed::Assistant,
        AgentEventKind::ToolInputDelta | AgentEventKind::ToolCall => Streamed::ToolInput,
        AgentEventKind::ReasoningSummaryDelta if scope.is_some() => Streamed::Reasoning,
        AgentEventKind::RunCompleted | AgentEventKind::RunFailed => {
            return Role::Boundary {
                scope,
                ends_run: true,
            };
        }
        AgentEventKind::RunError
        | AgentEventKind::RunSteered
        | AgentEventKind::ModelCallFailed
        | AgentEventKind::ModelAttemptFailed
        | AgentEventKind::ModelAttemptRetrying => {
            return Role::Boundary {
                scope,
                ends_run: false,
            };
        }
        AgentEventKind::ModelCallCompleted => return Role::Settled(scope),
        AgentEventKind::ModelCallStarted | AgentEventKind::ModelAttemptStarted => {
            return Role::Progress;
        }
        _ => return Role::Other,
    };
    let fields: StreamFields = serde_json::from_str(event.payload.get()).unwrap_or_default();
    let key = match streamed {
        Streamed::Assistant => fields.item_id,
        Streamed::ToolInput => fields.call_id,
        Streamed::Reasoning => fields.item_id.or_else(|| {
            fields
                .model_call_index
                .map(|index| format!("model-call-{index}"))
        }),
    };
    let Some(key) = key else {
        return Role::Other;
    };
    match (event.kind, fields.text) {
        (
            AgentEventKind::AssistantDelta
            | AgentEventKind::ToolInputDelta
            | AgentEventKind::ReasoningSummaryDelta,
            Some(text),
        ) => Role::Delta(scope, streamed, key, text),
        (
            AgentEventKind::AssistantDelta
            | AgentEventKind::ToolInputDelta
            | AgentEventKind::ReasoningSummaryDelta,
            None,
        ) => Role::Other,
        _ => Role::Final(scope, streamed, key),
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
        Ok(self
            .origin
            .event(rewritten(&self.event, Some(&self.text), self.offset)?))
    }
}

fn stream(
    streams: &mut Vec<Stream>,
    scope: Scope,
    kind: Streamed,
    key: String,
    text: &str,
    event: SessionEvent,
) -> Result<SessionEvent, serde_json::Error> {
    let (origin, event) = match Origin::of(event) {
        Ok(split) => split,
        Err(event) => return Ok(event),
    };
    let index = match streams
        .iter()
        .position(|stream| stream.scope == scope && stream.kind == kind && stream.key == key)
    {
        Some(index) => index,
        None => {
            streams.push(Stream {
                scope,
                kind,
                key,
                origin,
                event: event.clone(),
                text: String::new(),
                offset: 0,
            });
            streams.len() - 1
        }
    };
    let entry = &mut streams[index];
    let offset = entry.offset + utf16_len(&entry.text);
    entry.text.push_str(text);
    let live = rewritten(&event, None, offset)?;
    entry.event = event;
    Ok(entry.origin.event(live))
}

fn flush(
    streams: &mut Vec<Stream>,
    scope: &Scope,
    kind: Option<Streamed>,
    ends_run: bool,
) -> Result<Vec<SessionEvent>, serde_json::Error> {
    let mut merged = Vec::new();
    for entry in streams.iter_mut().filter(|entry| {
        entry.scope == *scope
            && kind.is_none_or(|kind| entry.kind == kind)
            && !entry.text.is_empty()
    }) {
        merged.push(entry.merged()?);
        entry.offset += utf16_len(&entry.text);
        entry.text.clear();
    }
    if ends_run {
        streams.retain(|entry| entry.scope != *scope);
    }
    Ok(merged)
}

#[derive(Deserialize)]
struct Nested {
    #[serde(default)]
    parent_call_id: Option<serde::de::IgnoredAny>,
}

#[derive(Default, Deserialize)]
struct Identifiers {
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct Envelope<'a> {
    #[serde(flatten)]
    pub(crate) event: &'a SessionEvent,
    #[serde(flatten)]
    pub(crate) annotation: &'a Map<String, Value>,
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

impl<L: ?Sized> Shared<L> {
    fn boundary(&self, event: &SessionEvent, request_id: Option<&str>) -> bool {
        let SessionEvent::Agent { event, .. } = event else {
            return false;
        };
        match event.kind {
            AgentEventKind::ToolCall => serde_json::from_str::<Nested>(event.payload.get())
                .is_ok_and(|nested| nested.parent_call_id.is_none()),
            AgentEventKind::RunSteered => true,
            AgentEventKind::RunCompleted | AgentEventKind::RunFailed => {
                !request_id.is_some_and(|id| {
                    self.state.read(|state| {
                        state
                            .requests
                            .get(id)
                            .is_some_and(|request| request.running == Some(Run::Inbox))
                    })
                })
            }
            _ => false,
        }
    }

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

struct LogSink<L: ?Sized>(Arc<Shared<L>>);

impl<L: EventLog + ?Sized> LogSink<L> {
    async fn persist(&self, events: Vec<SessionEvent>) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        if !self.0.recording.load(Ordering::Acquire) {
            return Ok(());
        }
        let annotation = self.0.host.annotate();
        let request_ids: Vec<_> = events.iter().map(|event| self.0.attribute(event)).collect();
        let boundary = self.0.log.tick().is_none()
            || events
                .iter()
                .zip(&request_ids)
                .any(|(event, request_id)| self.0.boundary(event, request_id.as_deref()));
        let read = |id: &str| {
            self.0
                .delivery
                .update(|delivery| delivery.mark(id, Delivered::Read))
        };
        let published = self
            .0
            .log
            .record(Recording {
                session_id: &self.0.session_id,
                annotation: &annotation,
                events: &events,
                request_ids: &request_ids,
                read: &read,
            })
            .await?;
        if boundary {
            self.0
                .log
                .flush()
                .await
                .map_err(|error| error.to_string())?;
        }
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
        deltas: bool,
        progress: bool,
    ) -> Result<Vec<(bool, Vec<SessionEvent>)>, String> {
        let encode = |error: serde_json::Error| format!("encode agent event: {error}");
        let mut segments: Vec<(bool, Vec<SessionEvent>)> = Vec::new();
        let mut push = |live: bool, event: SessionEvent| match segments.last_mut() {
            Some((kind, events)) if *kind == live => events.push(event),
            _ => segments.push((live, vec![event])),
        };
        for event in events {
            match role(&event) {
                Role::Delta(scope, kind, key, text) if deltas => {
                    push(
                        true,
                        stream(streams, scope, kind, key, &text, event).map_err(encode)?,
                    );
                }
                Role::Progress if progress => push(true, event),
                Role::Final(scope, kind, key) => {
                    for merged in
                        flush(streams, &scope, Some(Streamed::Reasoning), false).map_err(encode)?
                    {
                        push(false, merged);
                    }
                    streams.retain(|entry| {
                        entry.scope != scope || entry.kind != kind || entry.key != key
                    });
                    push(false, event);
                }
                Role::Settled(scope) => {
                    for merged in
                        flush(streams, &scope, Some(Streamed::Reasoning), false).map_err(encode)?
                    {
                        push(false, merged);
                    }
                    push(false, event);
                }
                Role::Boundary { scope, ends_run } => {
                    for merged in flush(streams, &scope, None, ends_run).map_err(encode)? {
                        push(false, merged);
                    }
                    push(false, event);
                }
                Role::Delta(..) | Role::Progress | Role::Other => push(false, event),
            }
        }
        Ok(segments)
    }
}

impl<L: EventLog + ?Sized> EventSink for LogSink<L> {
    fn coalesces(&self) -> bool {
        self.0.host.coalesce_deltas()
    }

    fn tick(&self) -> Option<Duration> {
        self.0.log.tick()
    }

    fn ticked(&self) -> HostFuture<'_, Result<(), String>> {
        Box::pin(async {
            if !self.0.recording.load(Ordering::Acquire) {
                return Ok(());
            }
            self.0.log.flush().await.map_err(|error| error.to_string())
        })
    }

    fn deliver(&self, events: Vec<SessionEvent>) -> HostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let (deltas, progress) = (self.0.host.live_deltas(), self.0.host.live_progress());
            if !deltas && !progress {
                return self.persist(events).await;
            }
            let segments = Self::route(
                &mut *self.0.streams.acquire().await,
                events,
                deltas,
                progress,
            )?;
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

struct Claim<'a, L: ?Sized>(&'a Shared<L>, &'a str);

impl<L: ?Sized> Drop for Claim<'_, L> {
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

pub struct Attach {
    pub runtime: AgentRuntime,
    pub agent: Agent,
    pub events: AgentEvents,
    pub subagents: Option<SubagentUpdates>,
    pub model: String,
}

pub struct TurnInbox<L: ?Sized> {
    shared: Arc<Shared<L>>,
    runtime: AgentRuntime,
    agent: Agent,
    events: SessionEvents,
    model: String,
    busy: AtomicBool,
}

impl<L: EventLog + ?Sized> TurnInbox<L> {
    pub fn attach(
        attach: Attach,
        log: Arc<L>,
        host: Arc<dyn InboxHost>,
        window: Duration,
    ) -> (Self, HostFuture<'static, ()>) {
        let Attach {
            runtime,
            agent,
            events,
            subagents,
            model,
        } = attach;
        let shared = Arc::new(Shared {
            log,
            host,
            state: Atom::default(),
            session_id: agent.session_id().to_owned(),
            streams: Owned::new(Vec::new()),
            delivery: Atom::default(),
            recording: AtomicBool::new(true),
        });
        let (events, pump) =
            SessionEvents::start(events, subagents, window, LogSink(Arc::clone(&shared)));
        (
            Self {
                shared,
                runtime,
                agent,
                events,
                model,
                busy: AtomicBool::new(false),
            },
            pump,
        )
    }

    #[must_use]
    pub const fn agent(&self) -> &Agent {
        &self.agent
    }

    #[must_use]
    pub fn log(&self) -> &Arc<L> {
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

    pub async fn settle(&self) -> Result<bool, InboxError> {
        self.events.drained().await.map_err(InboxError::Events)?;
        self.shared.log.flush().await?;
        if !self.idle() {
            return Ok(false);
        }
        Ok(self.runtime.settle(self.session_id()).await? && self.idle())
    }

    pub async fn shutdown(&self) -> Result<(), InboxError> {
        let shutdown = self.runtime.shutdown().await;
        let delivered = self.events.drained().await;
        let flushed = self.shared.log.flush().await;
        shutdown?;
        delivered.map_err(InboxError::Events)?;
        flushed.map_err(InboxError::Store)
    }

    pub async fn turn(&self, request_id: &str, mut request: PromptRequest, read: Vec<String>) {
        self.shared.delivery.set(Delivery(
            read.into_iter().map(|id| (id, Delivered::Read)).collect(),
        ));
        loop {
            let end = self.pass(request_id, request).await;
            let delivery = self.shared.delivery.get();
            match self.shared.host.next(request_id, end, &delivery).await {
                Next::Pass {
                    request: next,
                    read,
                } => {
                    self.shared.delivery.update(|delivery| {
                        for id in &read {
                            delivery.mark(id, Delivered::Read);
                        }
                    });
                    request = next;
                }
                Next::Done => return,
            }
        }
    }

    async fn pass(&self, request_id: &str, request: PromptRequest) -> PassEnd {
        self.shared.recording.store(true, Ordering::Release);
        let turn = match self.agent.prompt(request).await {
            Ok(turn) => turn,
            Err(error) => return PassEnd::Refused(error),
        };
        let control = turn.control();
        if let Some(note) = self
            .shared
            .host
            .passing(request_id, turn.request_id())
            .await
        {
            drop(control.steer(note).await);
        }
        let mut result = pin!(turn.result());
        let steering = || Halt::Stop("steering failed".to_owned());
        let mut halt = self
            .deliver(request_id, &control)
            .await
            .err()
            .map(|()| steering());
        while halt.is_none() {
            match select(result.as_mut(), self.shared.host.wake(request_id)).await {
                Either::Left((value, waking)) => {
                    drop(waking);
                    return self.passed(value, control).await;
                }
                Either::Right((wake, _)) => {
                    if wake == Wake::Inputs && self.deliver(request_id, &control).await.is_err() {
                        halt = Some(steering());
                    }
                    halt = halt.or(self.shared.host.check(request_id, wake).await);
                }
            }
        }
        self.stop_recording().await;
        match halt {
            Some(Halt::Stop(reason)) => PassEnd::Stopped { reason, control },
            _ => PassEnd::Left { control },
        }
    }

    async fn passed(
        &self,
        value: Result<TurnResult, NanocodeError>,
        control: TurnControl,
    ) -> PassEnd {
        if let Err(reason) = self.events.flushed().await {
            self.stop_recording().await;
            return PassEnd::Stopped { reason, control };
        }
        self.shared.delivery.update(Delivery::ended);
        match value {
            Ok(result) => PassEnd::Completed(result),
            Err(error) => PassEnd::Failed(error),
        }
    }

    async fn stop_recording(&self) {
        self.shared.recording.store(false, Ordering::Release);
        drop(self.events.drained().await);
    }

    async fn deliver(&self, request_id: &str, control: &TurnControl) -> Result<(), ()> {
        let inputs = self.shared.host.inputs(request_id).await.map_err(drop)?;
        for id in inputs {
            if self
                .shared
                .delivery
                .read(|delivery| delivery.of(&id) != Delivered::Ungiven)
            {
                continue;
            }
            let prompt = self
                .shared
                .host
                .input(request_id, &id)
                .await
                .map_err(drop)?;
            if control.steer_with_id(id.clone(), prompt).await.is_err() {
                break;
            }
            self.shared
                .delivery
                .update(|delivery| delivery.mark(&id, Delivered::Accepted));
        }
        Ok(())
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
}

impl<L: TurnLog + ?Sized> TurnInbox<L> {
    pub async fn open(
        runtime: AgentRuntime,
        store: &SessionStore,
        record: SessionRecord,
        log: Arc<L>,
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
        let (agent, events) = runtime.open_loaded(store, &stored).await?;
        agent.leave_interrupted_turns_to_host();
        let model = stored.summary.record.model.as_str().to_owned();
        Ok(Self::attach(
            Attach {
                runtime,
                agent,
                events,
                subagents,
                model,
            },
            log,
            host,
            window,
        ))
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
            let control = turn.control();
            ignore_inactive(control.cancel().await)?;
        }
        let message = turn.result().await?.into_final_message();
        self.agent.flush().await?;
        Ok(message)
    }
}
