use std::{
    future::poll_fn,
    pin::pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
    time::Duration,
};

use futures_util::future::select;
use nanocode_platform::Instant;
use nanocode_subagents::{AgentUpdate, ScopedAgentUpdate};
use nanocode_tool_host::embedded::HostFuture;
use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};

use crate::{
    AgentEvents, SubagentUpdates,
    events::{AgentEvent, AgentEventKind, AgentEventTiming, monotonic_now_ns},
};

const MERGED_FIELDS: [&str; 6] = [
    "item_id",
    "call_id",
    "tool",
    "turn_id",
    "model_call_index",
    "phase",
];

pub enum SessionEvent {
    Agent {
        event: AgentEvent,
        timing: AgentEventTiming,
        received_ns: u64,
    },
    Subagent(ScopedAgentUpdate),
}

pub trait EventSink: Send + Sync + 'static {
    fn deliver(&self, events: Vec<SessionEvent>) -> HostFuture<'_, Result<(), String>>;
}

type Drain = oneshot::Sender<Result<(), String>>;

#[derive(Clone)]
pub struct SessionEvents {
    requests: mpsc::UnboundedSender<Drain>,
    outcome: Arc<OnceLock<Result<(), String>>>,
}

struct Sources {
    agent: Option<AgentEvents>,
    subagents: Option<SubagentUpdates>,
}

const fn is_delta(kind: AgentEventKind) -> bool {
    matches!(
        kind,
        AgentEventKind::AssistantDelta
            | AgentEventKind::ReasoningSummaryDelta
            | AgentEventKind::ToolInputDelta
    )
}

impl SessionEvent {
    const fn delta(&self) -> Option<&AgentEvent> {
        match self {
            Self::Agent { event, .. } if is_delta(event.kind) => Some(event),
            Self::Subagent(ScopedAgentUpdate {
                update: AgentUpdate::Event { event, .. },
                ..
            }) if is_delta(event.kind) => Some(event),
            _ => None,
        }
    }

    const fn delta_mut(&mut self) -> Option<&mut AgentEvent> {
        match self {
            Self::Agent { event, .. } => Some(event),
            Self::Subagent(ScopedAgentUpdate {
                update: AgentUpdate::Event { event, .. },
                ..
            }) => Some(event),
            Self::Subagent(_) => None,
        }
    }

    fn same_source(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Agent { .. }, Self::Agent { .. }) => true,
            (
                Self::Subagent(ScopedAgentUpdate {
                    root_session_id: left_root,
                    update: AgentUpdate::Event { id: left, .. },
                    ..
                }),
                Self::Subagent(ScopedAgentUpdate {
                    root_session_id: right_root,
                    update: AgentUpdate::Event { id: right, .. },
                    ..
                }),
            ) => left_root == right_root && left == right,
            _ => false,
        }
    }
}

struct SdkTiming {
    emitted_ns: u64,
    received_ns: u64,
    source_received_ns: Option<u64>,
}

impl Serialize for SdkTiming {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("emitted_ns", &self.emitted_ns)?;
        map.serialize_entry("received_ns", &self.received_ns)?;
        map.serialize_entry(
            "queue_ns",
            &self.received_ns.saturating_sub(self.emitted_ns),
        )?;
        map.serialize_entry("source_received_ns", &self.source_received_ns)?;
        map.end()
    }
}

struct Update<'a>(&'a AgentUpdate);

impl Serialize for Update<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self.0 {
            AgentUpdate::Added(agent) => {
                map.serialize_entry("type", "added")?;
                map.serialize_entry("agent", agent)?;
            }
            AgentUpdate::Event { id, parent, event } => {
                map.serialize_entry("type", "event")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("parent", parent)?;
                map.serialize_entry("event", event)?;
            }
            AgentUpdate::Status { id, status } => {
                map.serialize_entry("type", "status")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("status", status)?;
            }
            AgentUpdate::Completion {
                id,
                status,
                revision,
            } => {
                map.serialize_entry("type", "completion")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("status", status)?;
                map.serialize_entry("revision", revision)?;
            }
            AgentUpdate::Message(message) => {
                map.serialize_entry("type", "message")?;
                map.serialize_entry("message", message)?;
            }
        }
        map.end()
    }
}

impl Serialize for SessionEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::Agent {
                event,
                timing,
                received_ns,
            } => {
                map.serialize_entry("source", "agent")?;
                map.serialize_entry("event", event)?;
                map.serialize_entry(
                    "sdk_timing",
                    &SdkTiming {
                        emitted_ns: timing.emitted_ns,
                        received_ns: *received_ns,
                        source_received_ns: timing.source_received_ns,
                    },
                )?;
            }
            Self::Subagent(update) => {
                map.serialize_entry("source", "subagent")?;
                map.serialize_entry("root_session_id", &update.root_session_id)?;
                map.serialize_entry("detach", &update.detach)?;
                map.serialize_entry("update", &Update(&update.update))?;
            }
        }
        map.end()
    }
}

fn payload(event: &AgentEvent) -> Option<Map<String, Value>> {
    match serde_json::from_str(event.payload.get()) {
        Ok(Value::Object(fields)) => Some(fields),
        _ => None,
    }
}

fn merge(previous: &mut SessionEvent, next: &SessionEvent) -> bool {
    let (Some(before), Some(after)) = (previous.delta(), next.delta()) else {
        return false;
    };
    if !previous.same_source(next)
        || before.kind != after.kind
        || before.request_id != after.request_id
    {
        return false;
    }
    let (Some(mut fields), Some(appended)) = (payload(before), payload(after)) else {
        return false;
    };
    if MERGED_FIELDS
        .iter()
        .any(|field| fields.get(*field) != appended.get(*field))
    {
        return false;
    }
    let (Some(Value::String(text)), Some(Value::String(more))) =
        (fields.get_mut("text"), appended.get("text"))
    else {
        return false;
    };
    text.push_str(more);
    let Ok(raw) = serde_json::value::to_raw_value(&fields) else {
        return false;
    };
    let Some(event) = previous.delta_mut() else {
        return false;
    };
    event.payload = Arc::from(raw);
    true
}

fn coalesce(batch: Vec<SessionEvent>) -> Vec<SessionEvent> {
    let mut merged: Vec<SessionEvent> = Vec::with_capacity(batch.len());
    for event in batch {
        if let Some(previous) = merged.last_mut()
            && merge(previous, &event)
        {
            continue;
        }
        merged.push(event);
    }
    merged
}

impl Sources {
    fn poll_batch(&mut self, context: &mut Context<'_>) -> Poll<Option<Vec<SessionEvent>>> {
        let mut batch = Vec::new();
        loop {
            let mut progressed = false;
            if let Some(agent) = &mut self.agent {
                match agent.poll_recv_timed(context) {
                    Poll::Ready(Some(timed)) => {
                        progressed = true;
                        batch.push(SessionEvent::Agent {
                            event: timed.event,
                            timing: timed.timing,
                            received_ns: monotonic_now_ns(),
                        });
                    }
                    Poll::Ready(None) => self.agent = None,
                    Poll::Pending => {}
                }
            }
            if let Some(subagents) = &mut self.subagents {
                match subagents.poll_recv(context) {
                    Poll::Ready(Some(update)) => {
                        progressed = true;
                        batch.push(SessionEvent::Subagent(update));
                    }
                    Poll::Ready(None) => self.subagents = None,
                    Poll::Pending => {}
                }
            }
            if !progressed {
                break;
            }
        }
        if !batch.is_empty() {
            Poll::Ready(Some(batch))
        } else if self.agent.is_none() && self.subagents.is_none() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

impl SessionEvents {
    pub fn start(
        agent: AgentEvents,
        subagents: Option<SubagentUpdates>,
        window: Duration,
        sink: impl EventSink,
    ) -> (Self, HostFuture<'static, ()>) {
        let (requests, drains) = mpsc::unbounded_channel();
        let outcome = Arc::new(OnceLock::new());
        let sources = Sources {
            agent: Some(agent),
            subagents,
        };
        let pump = Box::pin(run(sources, drains, Arc::clone(&outcome), window, sink));
        (Self { requests, outcome }, pump)
    }

    pub async fn drained(&self) -> Result<(), String> {
        if let Some(outcome) = self.outcome.get() {
            return outcome.clone();
        }
        let (sender, receiver) = oneshot::channel();
        if self.requests.send(sender).is_err() {
            return self.stopped();
        }
        match receiver.await {
            Ok(outcome) => outcome,
            Err(_) => self.stopped(),
        }
    }

    fn stopped(&self) -> Result<(), String> {
        self.outcome
            .get()
            .cloned()
            .unwrap_or_else(|| Err("agent event delivery stopped".into()))
    }
}

fn answer(waiting: &mut Vec<Drain>, outcome: &Result<(), String>) {
    for waiter in waiting.drain(..) {
        let _ = waiter.send(outcome.clone());
    }
}

async fn run(
    mut sources: Sources,
    mut drains: mpsc::UnboundedReceiver<Drain>,
    outcome: Arc<OnceLock<Result<(), String>>>,
    window: Duration,
    sink: impl EventSink,
) {
    let mut waiting = Vec::new();
    let mut flushed: Option<Instant> = None;
    let result = loop {
        let batch = poll_fn(|context| {
            while let Poll::Ready(Some(drain)) = drains.poll_recv(context) {
                waiting.push(drain);
            }
            let polled = sources.poll_batch(context);
            if polled.is_pending() {
                answer(&mut waiting, &Ok(()));
            }
            polled
        })
        .await;
        let Some(mut batch) = batch else {
            break Ok(());
        };
        if waiting.is_empty()
            && batch.last().is_some_and(|event| event.delta().is_some())
            && let Some(since) = flushed
                .map(|instant| instant.elapsed())
                .filter(|since| *since < window)
        {
            let requested = poll_fn(|context| match drains.poll_recv(context) {
                Poll::Ready(Some(drain)) => {
                    waiting.push(drain);
                    Poll::Ready(())
                }
                Poll::Ready(None) | Poll::Pending => Poll::Pending,
            });
            select(
                pin!(nanocode_platform::sleep(window - since)),
                pin!(requested),
            )
            .await;
            while let Poll::Ready(Some(more)) =
                poll_fn(|context| Poll::Ready(sources.poll_batch(context))).await
            {
                batch.extend(more);
            }
        }
        if let Err(error) = sink.deliver(coalesce(batch)).await {
            break Err(format!("persist agent events: {error}"));
        }
        flushed = Some(Instant::now());
    };
    let _ = outcome.set(result.clone());
    drains.close();
    while let Ok(drain) = drains.try_recv() {
        waiting.push(drain);
    }
    answer(&mut waiting, &result);
}
