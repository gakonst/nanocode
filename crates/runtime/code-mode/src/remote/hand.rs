use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

use futures_util::future::{AbortHandle, Abortable, select_all};
use nanocode_platform::{Duration, Instant};
use nanocode_sync::{Atom, Signal, Slot};
use nanocode_tool_contracts::RunningTask;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};

use super::{
    Answers, LOCAL_TOOLS_UNKNOWN, NOT_RECEIVED, REQUEST_IN_USE, RemoteContext,
    RemoteFrame as Frame, RemoteOperation, SentCalls, asked_key, encode_output, output_from_wire,
    output_to_wire,
};
use crate::{
    Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput,
    embedded::{
        CodeModeExecution, CodeModeHostError, CodeModeObserver, CodeModeUpdate, HandCalls,
        HostFuture, OwnedToolContext,
    },
};

pub const BRAIN_DISCONNECTED: &str = "the hand is not connected to its brain";

pub const CALL_CANCELLED: &str = "the call or its session was cancelled while it ran; its outcome is unknown and it was not retried";

const CALLBACK_DROPPED: &str = "the hand's connection to its brain dropped while the callback ran; its outcome is unknown and it was not retried";

const NO_CELLS: &str = "this hand has no cells for the session: it restarted or reconnected since the cell started, so the cell's outcome is unknown";

const DEFAULT_CALLBACK_HOLD: Duration = Duration::from_secs(60);

const MAX_BUFFERED_FRAMES: usize = 1024;

const WORK_PUBLISH_DELAY: Duration = Duration::from_millis(10);

fn error(value: impl std::fmt::Display) -> CodeModeHostError {
    CodeModeHostError::new(value.to_string())
}

#[derive(Clone, Default)]
pub struct ExecutorOptions {
    pub name: Option<String>,
    pub runtime_id: Option<String>,
    pub callback_timeout: Option<Duration>,
    pub callback_hold: Option<Duration>,
}

pub trait HandHost: Send + Sync + 'static {
    fn definitions(&self) -> HostFuture<'_, Vec<ToolDefinition>>;

    fn workspace(&self) -> Option<String>;

    fn selects_tools(&self) -> bool;

    fn open(
        &self,
        session_id: &str,
        excluded: Vec<String>,
        brain: HandBrain,
    ) -> Result<Arc<dyn HandSession>, CodeModeHostError>;

    fn end_turn<'a>(
        &'a self,
        session_id: &'a str,
        turn_id: &'a str,
        hook_event_name: &'a str,
    ) -> HostFuture<'a, ()>;

    fn spawn(&self, task: HostFuture<'static, ()>);
}

pub type DetachedObservers<'a> = &'a mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send);

pub trait HandSession: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        source: &'a str,
        context: OwnedToolContext,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>>;

    fn wait<'a>(
        &'a self,
        input: &'a str,
        context: OwnedToolContext,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>>;

    fn call<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: OwnedToolContext,
    ) -> HostFuture<'a, ToolOutput>;

    fn cancel<'a>(&'a self, observer: &'a mut dyn CodeModeObserver) -> HostFuture<'a, ()>;

    fn cancel_turn<'a>(&'a self, observer: &'a mut dyn CodeModeObserver) -> HostFuture<'a, ()>;

    fn detach_turn<'a>(&'a self, observers: DetachedObservers<'a>) -> HostFuture<'a, ()>;

    fn begin_turn(&self);

    fn preempt(&self) -> HostFuture<'_, ()>;

    fn settle_preempt(&self);

    fn running_tasks(&self) -> Vec<RunningTask> {
        Vec::new()
    }

    fn close(&self) -> HostFuture<'_, ()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BusyKind {
    Cell,
    Call,
    Task,
    Request,
    Reply,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusyItem {
    pub kind: BusyKind,
    pub id: String,
    pub label: String,
    pub session_id: Option<String>,
    pub silent_for: Duration,
}

impl BusyItem {
    fn task(task: RunningTask, session_id: Option<&str>) -> Self {
        Self {
            kind: BusyKind::Task,
            id: task.id,
            label: task.label,
            session_id: session_id.map(str::to_owned),
            silent_for: task.silent_for,
        }
    }
}

#[derive(Clone)]
pub struct HandActivity {
    shared: Arc<ActivityShared>,
}

struct ActivityShared {
    draining: AtomicBool,
    busy: watch::Sender<usize>,
    tools: Vec<Weak<dyn Tool>>,
    boards: Atom<Vec<Arc<Board>>>,
}

impl Default for HandActivity {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl HandActivity {
    #[must_use]
    pub fn new(tools: Vec<Weak<dyn Tool>>) -> Self {
        Self {
            shared: Arc::new(ActivityShared {
                draining: AtomicBool::new(false),
                busy: watch::channel(0).0,
                tools,
                boards: Atom::default(),
            }),
        }
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        *self.shared.busy.borrow() > 0
    }

    #[must_use]
    pub fn items(&self) -> Vec<BusyItem> {
        let mut items = Vec::new();
        for board in self.shared.boards.get().iter() {
            board.items(&mut items);
        }
        for tool in self.shared.tools.iter().filter_map(Weak::upgrade) {
            items.extend(
                tool.running_tasks()
                    .into_iter()
                    .map(|task| BusyItem::task(task, None)),
            );
        }
        items
    }

    pub async fn idle(&self) {
        let _ = self
            .shared
            .busy
            .subscribe()
            .wait_for(|busy| *busy == 0)
            .await;
    }

    pub fn drain(&self) {
        self.shared.draining.store(true, Ordering::Release);
    }

    pub fn undrain(&self) {
        self.shared.draining.store(false, Ordering::Release);
    }

    fn draining(&self) -> bool {
        self.shared.draining.load(Ordering::Acquire)
    }

    fn attach(&self) -> Arc<Board> {
        let board = Arc::<Board>::default();
        self.shared
            .boards
            .update(|boards| boards.push(Arc::clone(&board)));
        board
    }

    fn detach(&self, board: &Arc<Board>) {
        self.shared
            .boards
            .update(|boards| boards.retain(|other| !Arc::ptr_eq(other, board)));
    }

    fn report(&self, was: bool, now: bool) {
        if was != now {
            self.shared.busy.send_modify(|busy| {
                if now {
                    *busy += 1;
                } else {
                    *busy -= 1;
                }
            });
        }
    }
}

struct LastActivity {
    origin: Instant,
    last_us: AtomicU64,
}

impl LastActivity {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            last_us: AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        let elapsed = u64::try_from(self.origin.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.last_us.store(elapsed, Ordering::Relaxed);
    }

    fn silent_for(&self) -> Duration {
        self.origin
            .elapsed()
            .saturating_sub(Duration::from_micros(self.last_us.load(Ordering::Relaxed)))
    }
}

#[derive(Clone, Default)]
struct Cancel(Arc<CancelNode>);

#[derive(Default)]
struct CancelNode {
    signal: Signal,
    parent: Option<Cancel>,
}

impl Cancel {
    fn child(&self) -> Self {
        Self(Arc::new(CancelNode {
            signal: Signal::new(),
            parent: Some(self.clone()),
        }))
    }

    fn cancel(&self) {
        self.0.signal.fire();
    }

    async fn cancelled(&self) {
        let mut waits = Vec::new();
        let mut node = Some(self);
        while let Some(current) = node {
            waits.push(Box::pin(current.0.signal.wait()));
            node = current.0.parent.as_ref();
        }
        select_all(waits).await;
    }
}

#[derive(Clone)]
struct Outgoing {
    wire: Arc<Atom<Option<mpsc::UnboundedSender<String>>>>,
    limit: Arc<AtomicUsize>,
    backlog: Arc<Atom<VecDeque<Arc<str>>>>,
    flushing: Arc<AtomicBool>,
    resumed: Arc<AtomicBool>,
}

impl Outgoing {
    fn new() -> Self {
        Self {
            wire: Arc::default(),
            limit: Arc::new(AtomicUsize::new(usize::MAX)),
            backlog: Arc::default(),
            flushing: Arc::default(),
            resumed: Arc::default(),
        }
    }
    fn connect(&self, wire: mpsc::UnboundedSender<String>) {
        self.resumed.store(false, Ordering::SeqCst);
        self.wire.set(Some(wire));
    }
    fn clear_backlog(&self) {
        self.backlog.set(VecDeque::new());
    }
    fn queue(&self, text: String) {
        let text = if self.backlog.read(VecDeque::is_empty) && !self.flushing.load(Ordering::SeqCst)
        {
            match self.offer(text) {
                Ok(()) => return,
                Err((text, _)) => text,
            }
        } else {
            text
        };
        let text = Arc::<str>::from(text);
        self.backlog.update(|backlog| {
            if backlog.len() >= MAX_BUFFERED_FRAMES {
                backlog.pop_front();
            }
            backlog.push_back(Arc::clone(&text));
        });
        if self.resumed.load(Ordering::SeqCst) && self.claim() {
            let _ = self.drain();
        }
    }
    fn claim(&self) -> bool {
        self.flushing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
    fn flush(&self) -> Result<(), CodeModeHostError> {
        self.resumed.store(true, Ordering::SeqCst);
        if self.claim() { self.drain() } else { Ok(()) }
    }
    fn drain(&self) -> Result<(), CodeModeHostError> {
        loop {
            let mut frames = self.backlog.update(std::mem::take).into_iter();
            while let Some(text) = frames.next() {
                if let Err((_, issue)) = self.offer(text.to_string()) {
                    let unsent = std::iter::once(text).chain(frames).collect::<VecDeque<_>>();
                    self.backlog.update(|backlog| {
                        let mut held = unsent.clone();
                        held.extend(backlog.iter().cloned());
                        while held.len() > MAX_BUFFERED_FRAMES {
                            held.pop_front();
                        }
                        *backlog = held;
                    });
                    self.flushing.store(false, Ordering::SeqCst);
                    return Err(issue);
                }
            }
            self.flushing.store(false, Ordering::SeqCst);
            if self.backlog.read(VecDeque::is_empty) || !self.claim() {
                return Ok(());
            }
        }
    }
    fn encode(&self, frame: &Frame) -> Result<String, CodeModeHostError> {
        frame.encode(self.limit.load(Ordering::Relaxed))
    }
    fn connected(&self) -> bool {
        self.wire.get().is_some()
    }
    fn push(&self, text: String) -> Result<(), CodeModeHostError> {
        self.offer(text).map_err(|(_, issue)| issue)
    }
    fn offer(&self, text: String) -> Result<(), (String, CodeModeHostError)> {
        match Option::clone(&self.wire.get()) {
            Some(wire) => wire.send(text).map_err(|issue| {
                let reason = error(&issue);
                (issue.0, reason)
            }),
            None => Err((text, error(BRAIN_DISCONNECTED))),
        }
    }
    fn send(&self, frame: &Frame) -> Result<(), CodeModeHostError> {
        self.push(self.encode(frame)?)
    }
    fn send_or(
        &self,
        frame: &Frame,
        fallback: impl FnOnce(&CodeModeHostError) -> Frame,
    ) -> Result<bool, CodeModeHostError> {
        match self.encode(frame) {
            Ok(text) => self.push(text).map(|()| true),
            Err(issue) => self.send(&fallback(&issue)).map(|()| false),
        }
    }
}

fn unsent_update(update: &CodeModeUpdate<'_>, issue: &CodeModeHostError) -> Value {
    match update {
        CodeModeUpdate::NestedCallStarted { call_id, name, .. } => {
            json!({"call_id":call_id,"name":name,"input":Value::Null})
        }
        CodeModeUpdate::NestedCallCompleted(call) => json!({"completed":{
            "call_id":call.call_id,
            "name":call.name,
            "input":Value::Null,
            "output":format!("{issue}; the call's input and output were not sent"),
            "structured_result":Value::Null,
            "success":call.success,
            "started_after_ns":call.started_after_ns,
            "duration_ns":call.duration_ns,
            "metadata":Value::Null,
        }}),
        CodeModeUpdate::Notification(notification) => {
            json!({"notification":{"call_id":notification.call_id,"text":format!("{issue}; the notification was not sent")}})
        }
    }
}

fn status_line(execution: &CodeModeExecution) -> String {
    let header = match &execution.output {
        nanocode_tool_contracts::ToolOutputBody::Text(text) => Some(text.as_str()),
        nanocode_tool_contracts::ToolOutputBody::Content(content) => {
            content.first().and_then(|item| match item {
                nanocode_tool_contracts::ToolOutputContent::InputText { text } => {
                    Some(text.as_str())
                }
                _ => None,
            })
        }
    };
    header
        .and_then(|header| header.lines().next())
        .unwrap_or("Script failed")
        .to_owned()
}

fn reply_text(
    outgoing: &Outgoing,
    id: u64,
    result: Result<Option<Value>, String>,
    status: Option<&str>,
) -> Result<String, CodeModeHostError> {
    let mut frame = Frame::Reply { id, result };
    let mut issue = match outgoing.encode(&frame) {
        Ok(text) => return Ok(text),
        Err(issue) => issue,
    };
    let execution = match &mut frame {
        Frame::Reply {
            result: Ok(Some(Value::Object(execution))),
            ..
        } if status.is_some() => Some(execution),
        _ => None,
    };
    let Some(execution) = execution else {
        return outgoing.encode(&Frame::Reply {
            id,
            result: Err(format!("{issue}; the result was not sent")),
        });
    };
    if let Some(Value::Array(calls)) = execution.get_mut("nested_calls") {
        for call in calls
            .iter_mut()
            .filter_map(Value::as_object_mut)
            .filter(|call| !call.contains_key("delivered"))
        {
            call.insert("input".into(), Value::Null);
            call.insert(
                "output".into(),
                Value::String(format!(
                    "{issue}; the call's input and output were not sent"
                )),
            );
            call.insert("structured_result".into(), Value::Null);
            call.insert("metadata".into(), Value::Null);
        }
    }
    match outgoing.encode(&frame) {
        Ok(text) => return Ok(text),
        Err(next) => issue = next,
    }
    if let Frame::Reply {
        result: Ok(Some(Value::Object(execution))),
        ..
    } = &mut frame
    {
        let status = status.unwrap_or_default();
        execution.insert(
            "output".into(),
            Value::String(format!(
                "{status}\nOutput:\n{issue}; the script's output was not sent"
            )),
        );
        execution.insert("success".into(), Value::Bool(false));
        execution.insert("notifications".into(), json!([]));
    }
    outgoing.encode(&frame)
}

fn origin(call_id: &str) -> &str {
    call_id
        .rsplit_once("/code-")
        .map_or(call_id, |(origin, _)| origin)
}

fn update_call_id<'a>(update: &CodeModeUpdate<'a>) -> &'a str {
    match update {
        CodeModeUpdate::NestedCallStarted { call_id, .. } => call_id,
        CodeModeUpdate::NestedCallCompleted(call) => &call.call_id,
        CodeModeUpdate::Notification(notification) => &notification.call_id,
    }
}

struct Observer {
    id: u64,
    outgoing: Outgoing,
    catalog: CallbackCatalog,
    calls: SentCalls,
}

impl CodeModeObserver for Observer {
    fn update(&mut self, update: CodeModeUpdate<'_>) {
        self.catalog.touch(update_call_id(&update));
        let id = self.id;
        let sent = self.outgoing.send_or(
            &Frame::Update {
                id,
                update: self.calls.update(&update),
            },
            |issue| Frame::Update {
                id,
                update: unsent_update(&update, issue),
            },
        );
        if matches!(sent, Ok(true)) {
            self.calls.sent(&update);
        }
    }
}

struct DetachedObserver {
    session_id: String,
    origin_call_id: String,
    outgoing: Outgoing,
    catalog: CallbackCatalog,
    calls: SentCalls,
}

impl CodeModeObserver for DetachedObserver {
    fn update(&mut self, update: CodeModeUpdate<'_>) {
        self.catalog.touch(&self.origin_call_id);
        let frame = |update| Frame::DetachedUpdate {
            session_id: self.session_id.clone(),
            origin_call_id: self.origin_call_id.clone(),
            update,
        };
        let text = self
            .outgoing
            .encode(&frame(self.calls.update(&update)))
            .or_else(|issue| self.outgoing.encode(&frame(unsent_update(&update, &issue))));
        if let Ok(text) = text {
            self.outgoing.queue(text);
        }
    }

    fn completed(&mut self) {
        self.catalog.finish(&self.origin_call_id);
    }
}

type CallbackReply = Arc<Slot<oneshot::Sender<Result<Value, String>>>>;

#[derive(Clone)]
struct PendingCallback {
    reply: CallbackReply,
    frame: String,
    held: bool,
}

struct CallbackBus {
    outgoing: Outgoing,
    pending: Atom<HashMap<u64, PendingCallback>>,
    next: AtomicU64,
    timeout: Option<Duration>,
    holding: AtomicBool,
    answers: Arc<Answers>,
    answering: AtomicBool,
    asking: AtomicBool,
}

impl CallbackBus {
    fn hold(&self) {
        self.pending.update(|pending| {
            for entry in pending.values_mut() {
                entry.held = true;
            }
        });
        self.holding.store(true, Ordering::SeqCst);
    }

    fn take_held(&self) -> Vec<String> {
        self.holding.store(false, Ordering::SeqCst);
        self.pending.update(|pending| {
            let mut held = pending
                .iter_mut()
                .filter(|(_, entry)| entry.held)
                .map(|(id, entry)| {
                    entry.held = false;
                    (*id, entry.frame.clone())
                })
                .collect::<Vec<_>>();
            held.sort_unstable_by_key(|(id, _)| *id);
            held.into_iter().map(|(_, frame)| frame).collect()
        })
    }

    fn fail_pending(&self) {
        self.holding.store(false, Ordering::SeqCst);
        for (_, entry) in self.pending.update(std::mem::take) {
            if let Some(reply) = entry.reply.take() {
                let _ = reply.send(Err(CALLBACK_DROPPED.into()));
            }
        }
    }

    fn answer(&self, id: u64, result: Result<Value, String>) {
        if let Some(reply) = self
            .pending
            .update(|pending| pending.remove(&id))
            .and_then(|entry| entry.reply.take())
        {
            let _ = reply.send(result);
        }
    }

    fn idle(&self) -> bool {
        self.pending.read(HashMap::is_empty)
    }
}

struct CallbackPending {
    bus: Arc<CallbackBus>,
    id: u64,
}

impl Drop for CallbackPending {
    fn drop(&mut self) {
        if self
            .bus
            .pending
            .update(|pending| pending.remove(&self.id))
            .is_some()
        {
            let _ = self
                .bus
                .outgoing
                .send(&Frame::CallbackCancel { id: self.id });
        }
    }
}

async fn callback(
    bus: &Arc<CallbackBus>,
    recorded: Option<String>,
    asked: Option<(String, Value)>,
    frame: impl FnOnce(u64) -> Frame,
) -> Result<ToolOutput, CodeModeHostError> {
    let id = bus.next.fetch_add(1, Ordering::Relaxed);
    let text = bus
        .outgoing
        .encode(&frame(id))
        .map_err(|issue| error(format!("{issue}; the call was not dispatched")))?;
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Slot::new(Some(tx)));
    let held = !bus.outgoing.connected();
    bus.pending.update(|pending| {
        pending.insert(
            id,
            PendingCallback {
                reply: Arc::clone(&tx),
                frame: text.clone(),
                held,
            },
        );
    });
    let _pending = CallbackPending {
        bus: Arc::clone(bus),
        id,
    };
    if held {
        if !bus.holding.load(Ordering::SeqCst) {
            return Err(error(BRAIN_DISCONNECTED));
        }
    } else {
        bus.outgoing.push(text)?;
        if let Some((key, input)) = asked {
            bus.answers.keep(key, input);
        }
    }
    let result = match bus.timeout {
        Some(deadline) => nanocode_platform::timeout(deadline, rx)
            .await
            .map_err(|_| error("brain callback deadline elapsed; outcome unknown"))?,
        None => rx.await,
    };
    let value = result.map_err(error)?.map_err(error)?;
    let output = output_from_wire(value)?;
    if let Some(recorded) = recorded {
        bus.answers.record(recorded, &output);
    }
    Ok(output)
}

struct CallbackTool {
    definition: ToolDefinition,
    bus: Arc<CallbackBus>,
}

impl Tool for CallbackTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> HostFuture<'async_trait, nanocode_tool_contracts::ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        let context = RemoteContext::from_context(context);
        Box::pin(async move {
            let (input, freeform) = match input {
                ToolInput::Function(input) => (serde_json::from_str(input.get())?, false),
                ToolInput::Freeform(input) => (Value::String(input), true),
            };
            let answers = self.bus.answering.load(Ordering::SeqCst);
            let recorded = answers.then(|| context.call_id.clone());
            let asking = self.bus.asking.load(Ordering::SeqCst);
            let asked = asking.then(|| (asked_key(&context.call_id), input.clone()));
            callback(&self.bus, recorded, asked, |id| Frame::Callback {
                id,
                name: self.definition.name().into(),
                input,
                freeform,
                context,
                hand: None,
                compact: true,
                output_prefix: true,
                answers,
                asked: asking,
            })
            .await
            .map_err(|issue| issue.to_string().into())
        })
    }
}

struct HandCallbacks {
    bus: Arc<CallbackBus>,
    name: Atom<Option<String>>,
}

impl HandCalls for HandCallbacks {
    fn here(&self, hand: &str) -> bool {
        self.name.read(|name| name.as_deref() == Some(hand))
    }

    fn call<'a>(
        &'a self,
        hand: &'a str,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput> {
        let context = RemoteContext::from_context(context);
        Box::pin(async move {
            let answers = self.bus.answering.load(Ordering::SeqCst);
            let recorded = answers.then(|| context.call_id.clone());
            let asking = self.bus.asking.load(Ordering::SeqCst);
            let asked = asking.then(|| {
                let mut kept = input.clone();
                if let Value::Object(fields) = &mut kept {
                    fields.insert("hand".into(), Value::String(hand.into()));
                }
                (asked_key(&context.call_id), kept)
            });
            callback(&self.bus, recorded, asked, |id| Frame::Callback {
                id,
                name: name.into(),
                input,
                freeform: false,
                context,
                hand: Some(hand.into()),
                compact: true,
                output_prefix: true,
                answers,
                asked: asking,
            })
            .await
            .unwrap_or_else(|error| ToolOutput::error(error.to_string()))
        })
    }
}

#[derive(Clone)]
struct CallbackOrigin {
    id: String,
    label: String,
    activity: Arc<LastActivity>,
    turn: u64,
    definitions: Vec<ToolDefinition>,
}

#[derive(Clone, Default)]
struct CallbackCatalogState {
    turn: u64,
    origins: Vec<CallbackOrigin>,
    selected_tools: Option<Vec<String>>,
}

#[derive(Clone)]
struct CallbackCatalog {
    session_id: Arc<str>,
    state: Arc<Atom<CallbackCatalogState>>,
    bus: Arc<CallbackBus>,
}

impl CallbackCatalog {
    fn register(
        &self,
        origin: &str,
        label: &str,
        definitions: Vec<ToolDefinition>,
    ) -> Result<(), CodeModeHostError> {
        let activity = Arc::new(LastActivity::new());
        self.state.modify(|state| {
            if state.origins.iter().any(|entry| entry.id == origin) {
                return Err(error("callback origin is already active"));
            }
            let turn = state.turn;
            state.origins.push(CallbackOrigin {
                id: origin.into(),
                label: label.into(),
                activity: Arc::clone(&activity),
                turn,
                definitions: definitions.clone(),
            });
            Ok(())
        })
    }

    fn touch(&self, call_id: &str) {
        let origin = origin(call_id);
        self.state.read(|state| {
            if let Some(entry) = state.origins.iter().find(|entry| entry.id == origin) {
                entry.activity.touch();
            }
        });
    }

    fn turn(&self) -> u64 {
        self.state.read(|state| state.turn)
    }

    fn begin_turn(&self) {
        self.state.update(|state| state.turn += 1);
    }

    fn adopt(&self, origin: &str, turn: u64) {
        self.state.update(|state| {
            if let Some(entry) = state.origins.iter_mut().find(|entry| entry.id == origin) {
                entry.turn = turn;
            }
        });
    }

    fn forget(&self, origin: &str) -> bool {
        self.bus.answers.forget(&format!("{origin}/"));
        self.state
            .try_update(|state| {
                let index = state.origins.iter().position(|entry| entry.id == origin)?;
                state.origins.remove(index);
                Some(())
            })
            .is_some()
    }

    fn finish(&self, origin: &str) {
        if self.forget(origin)
            && let Ok(text) = self.bus.outgoing.encode(&Frame::DetachedComplete {
                session_id: self.session_id.to_string(),
                origin_call_id: origin.into(),
            })
        {
            self.bus.outgoing.queue(text);
        }
    }

    fn finish_turn(&self, turn: Option<u64>) {
        let origins = self.state.read(|state| {
            state
                .origins
                .iter()
                .filter(|entry| turn.is_none_or(|turn| entry.turn == turn))
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>()
        });
        for origin in origins {
            self.finish(&origin);
        }
    }

    fn definition(&self, origin: &str, name: &str) -> Option<ToolDefinition> {
        self.state.read(|state| {
            state
                .origins
                .iter()
                .find(|entry| entry.id == origin)?
                .definitions
                .iter()
                .find(|definition| definition.name() == name)
                .cloned()
        })
    }
}

#[derive(Clone)]
pub struct HandBrain {
    catalog: CallbackCatalog,
    hands: Arc<HandCallbacks>,
}

impl HandBrain {
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let state = self.catalog.state.get();
        let mut names = HashSet::new();
        state
            .origins
            .iter()
            .rev()
            .flat_map(|entry| entry.definitions.iter())
            .filter(|definition| names.insert(definition.name().to_owned()))
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn tools(&self, call_id: &str) -> Vec<Arc<dyn Tool>> {
        let origin = origin(call_id);
        self.catalog.state.read(|state| {
            state
                .origins
                .iter()
                .find(|entry| entry.id == origin)
                .map(|entry| {
                    entry
                        .definitions
                        .iter()
                        .map(|definition| {
                            Arc::new(CallbackTool {
                                definition: definition.clone(),
                                bus: Arc::clone(&self.catalog.bus),
                            }) as Arc<dyn Tool>
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    pub async fn execute(
        &self,
        name: &str,
        input: Value,
        context: ToolContext<'_>,
    ) -> Option<ToolOutput> {
        let definition = self.catalog.definition(origin(context.call_id()), name)?;
        let input = match &definition {
            ToolDefinition::Custom { .. } => match input.as_str() {
                Some(source) => ToolInput::Freeform(source.into()),
                None => return Some(ToolOutput::error("custom callback input must be a string")),
            },
            _ => match serde_json::value::to_raw_value(&input) {
                Ok(raw) => ToolInput::Function(raw),
                Err(error) => return Some(ToolOutput::error(error.to_string())),
            },
        };
        let proxy = CallbackTool {
            definition,
            bus: Arc::clone(&self.catalog.bus),
        };
        Some(match proxy.execute(input, context).await {
            Ok(output) => output,
            Err(error) => ToolOutput::error(error.to_string()),
        })
    }

    #[must_use]
    pub fn hand_calls(&self) -> Arc<dyn HandCalls> {
        self.hands.clone()
    }
}

struct OriginGuard {
    catalog: CallbackCatalog,
    origin: String,
    replied: bool,
}

impl Drop for OriginGuard {
    fn drop(&mut self) {
        if !self.replied {
            self.catalog.finish(&self.origin);
        }
    }
}

type ReplyResult = Result<Option<Value>, String>;

struct Done {
    id: u64,
    live: ReplyResult,
    full: Option<Value>,
    status: Option<String>,
}

struct Running {
    resumable: bool,
    connection: u64,
    work: Arc<Pending>,
    ended: Arc<AtomicBool>,
    cancel: Option<Cancel>,
}

struct Retained {
    text: String,
    work: Arc<Pending>,
    ended: Arc<AtomicBool>,
}

#[derive(Clone)]
struct Pending {
    kind: BusyKind,
    id: String,
    label: String,
    session_id: String,
    since: Instant,
}

impl Pending {
    fn new(id: u64, operation: &RemoteOperation) -> Self {
        let request = |label: &str, session_id: &str| {
            (
                BusyKind::Request,
                id.to_string(),
                label.to_owned(),
                session_id.to_owned(),
            )
        };
        let (kind, id, label, session_id) = match operation {
            RemoteOperation::Execute {
                source, context, ..
            } => (
                BusyKind::Cell,
                context.call_id.clone(),
                source
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty() && !line.starts_with("// @exec:"))
                    .unwrap_or_default()
                    .to_owned(),
                context.session_id.clone(),
            ),
            RemoteOperation::Call { name, context, .. } => (
                BusyKind::Call,
                context.call_id.clone(),
                name.clone(),
                context.session_id.clone(),
            ),
            RemoteOperation::Wait { context, .. } => request("wait", &context.session_id),
            RemoteOperation::DetachTurn { session_id } => request("detach turn", session_id),
            RemoteOperation::CancelTurn { session_id, .. } => request("cancel turn", session_id),
            RemoteOperation::Cancel { session_id } => request("cancel", session_id),
            RemoteOperation::CancelCall { session_id, .. } => request("cancel call", session_id),
            RemoteOperation::BeginTurn { session_id } => request("begin turn", session_id),
            RemoteOperation::Preempt { session_id } => request("preempt", session_id),
            RemoteOperation::SettlePreempt { session_id } => request("settle preempt", session_id),
            RemoteOperation::EndTurn { session_id, .. } => request("end turn", session_id),
        };
        Self {
            kind,
            id,
            label,
            session_id,
            since: Instant::now(),
        }
    }

    fn item(&self) -> BusyItem {
        BusyItem {
            kind: self.kind,
            id: self.id.clone(),
            label: self.label.clone(),
            session_id: Some(self.session_id.clone()),
            silent_for: self.since.elapsed(),
        }
    }
}

struct Listed {
    work: Arc<Pending>,
    ended: Arc<AtomicBool>,
    next: Option<Arc<Self>>,
}

impl Drop for Listed {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(node) = next {
            next = Arc::try_unwrap(node)
                .ok()
                .and_then(|mut node| node.next.take());
        }
    }
}

type Session = (Arc<dyn HandSession>, CallbackCatalog);

type Listing = (String, Arc<dyn HandSession>, CallbackCatalog);

#[derive(Default)]
struct Board {
    sessions: Atom<Vec<Listing>>,
    work: Atom<Option<Arc<Listed>>>,
}

impl Board {
    fn items(&self, items: &mut Vec<BusyItem>) {
        for (session_id, session, catalog) in self.sessions.get().iter() {
            catalog.state.read(|state| {
                items.extend(state.origins.iter().map(|origin| BusyItem {
                    kind: BusyKind::Cell,
                    id: origin.id.clone(),
                    label: origin.label.clone(),
                    session_id: Some(session_id.clone()),
                    silent_for: origin.activity.silent_for(),
                }));
            });
            items.extend(
                session
                    .running_tasks()
                    .into_iter()
                    .map(|task| BusyItem::task(task, Some(session_id))),
            );
        }
        let mut next = Option::clone(&self.work.get());
        while let Some(node) = next {
            next = node.next.clone();
            if node.ended.load(Ordering::Acquire) {
                continue;
            }
            let work = &node.work;
            let listed = work.kind == BusyKind::Cell
                && items.iter().any(|item| {
                    item.kind == BusyKind::Cell
                        && item.id == work.id
                        && item.session_id.as_deref() == Some(work.session_id.as_str())
                });
            if !listed {
                items.push(work.item());
            }
        }
    }
}

enum Event {
    Catalog {
        max_frame_bytes: usize,
        reply: oneshot::Sender<Result<String, CodeModeHostError>>,
    },
    Connect {
        connection: u64,
        wire: mpsc::UnboundedSender<String>,
        max_frame_bytes: usize,
        ended: oneshot::Sender<CodeModeHostError>,
    },
    Frame {
        connection: u64,
        frame: Frame,
    },
    Closed {
        connection: u64,
        reply: oneshot::Sender<()>,
    },
    Reset {
        reply: oneshot::Sender<()>,
    },
}

pub struct HandConnection {
    pub id: u64,
    pub failed: oneshot::Receiver<CodeModeHostError>,
}

#[derive(Clone)]
pub struct HandCore {
    events: mpsc::UnboundedSender<Event>,
    connections: Arc<AtomicU64>,
}

impl HandCore {
    pub fn new(
        host: Arc<dyn HandHost>,
        options: ExecutorOptions,
        activity: HandActivity,
    ) -> (Self, HostFuture<'static, ()>) {
        let (events, inbox) = mpsc::unbounded_channel();
        let state = State::new(host, options, activity);
        (
            Self {
                events,
                connections: Arc::default(),
            },
            Box::pin(drive(state, inbox)),
        )
    }

    pub async fn catalog(&self, max_frame_bytes: usize) -> Result<String, CodeModeHostError> {
        let (reply, answer) = oneshot::channel();
        self.events
            .send(Event::Catalog {
                max_frame_bytes,
                reply,
            })
            .map_err(|_| error("the hand stopped"))?;
        answer.await.map_err(|_| error("the hand stopped"))?
    }

    #[must_use]
    pub fn connect(
        &self,
        wire: mpsc::UnboundedSender<String>,
        max_frame_bytes: usize,
    ) -> HandConnection {
        let connection = self.connections.fetch_add(1, Ordering::Relaxed) + 1;
        let (ended, failed) = oneshot::channel();
        let _ = self.events.send(Event::Connect {
            connection,
            wire,
            max_frame_bytes,
            ended,
        });
        HandConnection {
            id: connection,
            failed,
        }
    }

    pub fn receive(&self, connection: u64, frame: Frame) {
        let _ = self.events.send(Event::Frame { connection, frame });
    }

    pub async fn closed(&self, connection: u64) {
        let (reply, done) = oneshot::channel();
        if self
            .events
            .send(Event::Closed { connection, reply })
            .is_ok()
        {
            let _ = done.await;
        }
    }

    pub async fn reset(&self) {
        let (reply, done) = oneshot::channel();
        if self.events.send(Event::Reset { reply }).is_ok() {
            let _ = done.await;
        }
    }
}

async fn due(sleep: &mut Option<HostFuture<'static, ()>>) {
    match sleep {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending().await,
    }
}

async fn drive(mut state: State, mut inbox: mpsc::UnboundedReceiver<Event>) {
    loop {
        state.publish();
        tokio::select! {
            event = inbox.recv() => match event {
                Some(event) => state.event(event).await,
                None => break,
            },
            Some(done) = state.results.recv() => {
                if let Err(issue) = state.deliver(done) {
                    tracing::error!(%issue, "remote reply could not be encoded");
                }
            },
            () = due(&mut state.work_due) => state.publish_work(),
        }
    }
    state.reset().await;
}

struct State {
    host: Arc<dyn HandHost>,
    options: ExecutorOptions,
    definitions: Option<Vec<ToolDefinition>>,
    outgoing: Outgoing,
    bus: Arc<CallbackBus>,
    callbacks: Arc<HandCallbacks>,
    sessions: HashMap<String, Session>,
    tasks: HashMap<u64, AbortHandle>,
    finished: mpsc::UnboundedSender<Done>,
    results: mpsc::UnboundedReceiver<Done>,
    running: HashMap<u64, Running>,
    calls: HashMap<String, Cancel>,
    turn_calls: HashMap<(String, String), Cancel>,
    retained: HashMap<u64, Retained>,
    brain_tools: HashMap<String, Vec<ToolDefinition>>,
    earlier_brain_tools: HashMap<String, Vec<ToolDefinition>>,
    connection: u64,
    ended: Option<oneshot::Sender<CodeModeHostError>>,
    resumable: bool,
    activity: HandActivity,
    board: Arc<Board>,
    work: Option<Arc<Listed>>,
    work_due: Option<HostFuture<'static, ()>>,
    listed: usize,
    unlisted: usize,
    busy: bool,
    hold: Option<AbortHandle>,
}

impl State {
    fn new(host: Arc<dyn HandHost>, options: ExecutorOptions, activity: HandActivity) -> Self {
        let outgoing = Outgoing::new();
        let bus = Arc::new(CallbackBus {
            outgoing: outgoing.clone(),
            pending: Atom::default(),
            next: AtomicU64::new(0),
            timeout: options.callback_timeout,
            holding: AtomicBool::new(false),
            answers: Arc::default(),
            answering: AtomicBool::new(false),
            asking: AtomicBool::new(false),
        });
        let callbacks = Arc::new(HandCallbacks {
            bus: Arc::clone(&bus),
            name: Atom::new(options.name.clone()),
        });
        let (finished, results) = mpsc::unbounded_channel();
        let board = activity.attach();
        Self {
            host,
            options,
            definitions: None,
            outgoing,
            bus,
            callbacks,
            sessions: HashMap::new(),
            tasks: HashMap::new(),
            finished,
            results,
            running: HashMap::new(),
            calls: HashMap::new(),
            turn_calls: HashMap::new(),
            retained: HashMap::new(),
            brain_tools: HashMap::new(),
            earlier_brain_tools: HashMap::new(),
            connection: 0,
            ended: None,
            resumable: false,
            activity,
            board,
            work: None,
            work_due: None,
            listed: 0,
            unlisted: 0,
            busy: false,
            hold: None,
        }
    }

    async fn event(&mut self, event: Event) {
        match event {
            Event::Catalog {
                max_frame_bytes,
                reply,
            } => {
                let _ = reply.send(self.catalog(max_frame_bytes).await);
            }
            Event::Connect {
                connection,
                wire,
                max_frame_bytes,
                ended,
            } => {
                self.connection = connection;
                self.ended = Some(ended);
                self.outgoing
                    .limit
                    .store(max_frame_bytes, Ordering::Relaxed);
                self.outgoing.connect(wire);
                self.earlier_brain_tools = std::mem::take(&mut self.brain_tools);
            }
            Event::Frame { connection, frame } => {
                if connection != self.connection {
                    return;
                }
                if let Err(issue) = self.frame(frame).await
                    && let Some(ended) = self.ended.take()
                {
                    self.outgoing.wire.set(None);
                    let _ = ended.send(issue);
                }
            }
            Event::Closed { connection, reply } => {
                if connection == self.connection {
                    self.ended = None;
                    self.disconnected();
                    if !self.resumable {
                        self.reset().await;
                    }
                    self.publish();
                }
                let _ = reply.send(());
            }
            Event::Reset { reply } => {
                self.reset().await;
                let _ = reply.send(());
            }
        }
    }

    async fn catalog(&mut self, max_frame_bytes: usize) -> Result<String, CodeModeHostError> {
        Frame::Catalog {
            definitions: self.definitions().await,
            name: self.options.name.clone(),
            workspace: self.host.workspace(),
            runtime_id: self.options.runtime_id.clone(),
            max_frame_bytes: Some(max_frame_bytes),
            resumable: self.options.runtime_id.is_some(),
            tool_selection: self.host.selects_tools(),
            cancels_calls: true,
            caches_local_tools: true,
        }
        .encode(max_frame_bytes)
    }

    async fn definitions(&mut self) -> Vec<ToolDefinition> {
        if let Some(definitions) = &self.definitions {
            return definitions.clone();
        }
        let definitions = self.host.definitions().await;
        self.definitions = Some(definitions.clone());
        definitions
    }

    fn spawn(&mut self, id: u64, task: HostFuture<'static, ()>) {
        let (handle, registration) = AbortHandle::new_pair();
        self.tasks.insert(id, handle);
        self.host.spawn(Box::pin(async move {
            let _ = Abortable::new(task, registration).await;
        }));
    }

    fn list(&mut self, work: &Arc<Pending>) -> Arc<AtomicBool> {
        let ended = Arc::new(AtomicBool::new(false));
        self.work = Some(Arc::new(Listed {
            work: Arc::clone(work),
            ended: Arc::clone(&ended),
            next: self.work.take(),
        }));
        self.listed += 1;
        self.work_changed();
        ended
    }

    fn work_changed(&mut self) {
        if self.work_due.is_none() {
            self.work_due = Some(Box::pin(nanocode_platform::sleep(WORK_PUBLISH_DELAY)));
        }
    }

    fn publish_work(&mut self) {
        self.work_due = None;
        self.board.work.set(self.work.clone());
    }

    fn unlist(&mut self, ended: &AtomicBool) {
        ended.store(true, Ordering::Release);
        self.unlisted += 1;
        if self.unlisted * 2 <= self.listed {
            return;
        }
        let mut live = Vec::new();
        let mut next = self.work.clone();
        while let Some(node) = next {
            if !node.ended.load(Ordering::Acquire) {
                live.push((Arc::clone(&node.work), Arc::clone(&node.ended)));
            }
            next = node.next.clone();
        }
        self.listed = live.len();
        self.unlisted = 0;
        let mut head = None;
        for (work, ended) in live.into_iter().rev() {
            head = Some(Arc::new(Listed {
                work,
                ended,
                next: head,
            }));
        }
        self.work = head;
        self.work_changed();
    }

    fn publish_sessions(&self) {
        self.board.sessions.set(
            self.sessions
                .iter()
                .map(|(session_id, (session, catalog))| {
                    (session_id.clone(), Arc::clone(session), catalog.clone())
                })
                .collect(),
        );
    }

    fn publish(&mut self) {
        let busy = !self.running.is_empty()
            || !self.retained.is_empty()
            || !self.bus.idle()
            || self
                .sessions
                .values()
                .any(|(_, catalog)| catalog.state.read(|state| !state.origins.is_empty()));
        self.activity.report(self.busy, busy);
        self.busy = busy;
    }

    fn disconnected(&mut self) {
        self.outgoing.wire.set(None);
        self.bus.hold();
        let bus = Arc::clone(&self.bus);
        let hold = self.options.callback_hold.unwrap_or(DEFAULT_CALLBACK_HOLD);
        let (handle, registration) = AbortHandle::new_pair();
        self.host.spawn(Box::pin(async move {
            if Abortable::new(nanocode_platform::sleep(hold), registration)
                .await
                .is_ok()
            {
                bus.fail_pending();
            }
        }));
        if let Some(previous) = self.hold.replace(handle) {
            previous.abort();
        }
        self.publish_work();
    }

    async fn reset(&mut self) {
        self.outgoing.wire.set(None);
        if let Some(timer) = self.hold.take() {
            timer.abort();
        }
        self.bus.fail_pending();
        self.outgoing.clear_backlog();
        for (_, task) in self.tasks.drain() {
            task.abort();
        }
        for (_, (session, _)) in std::mem::take(&mut self.sessions) {
            session.close().await;
        }
        while self.results.try_recv().is_ok() {}
        self.running.clear();
        self.calls.clear();
        self.turn_calls.clear();
        self.retained.clear();
        self.work = None;
        self.publish_work();
        self.listed = 0;
        self.unlisted = 0;
        self.publish_sessions();
        self.resumable = false;
        self.publish();
    }

    fn finish(&self, id: u64, live: ReplyResult) {
        let _ = self.finished.send(Done {
            id,
            live,
            full: None,
            status: None,
        });
    }

    fn deliver(&mut self, done: Done) -> Result<(), CodeModeHostError> {
        let Done {
            id,
            live,
            full,
            status,
        } = done;
        self.tasks.remove(&id);
        let running = self.running.remove(&id);
        let reconnected = running
            .as_ref()
            .is_some_and(|running| running.connection != self.connection);
        let status = status.as_deref();
        let refused = matches!(&live, Err(message) if message == NOT_RECEIVED);
        let text = match running {
            Some(running) if running.resumable && !refused => {
                let (retained, text) = match full {
                    Some(full) if !reconnected => (
                        reply_text(&self.outgoing, id, Ok(Some(full)), status)?,
                        reply_text(&self.outgoing, id, live, status)?,
                    ),
                    Some(full) => {
                        let retained = reply_text(&self.outgoing, id, Ok(Some(full)), status)?;
                        (retained.clone(), retained)
                    }
                    None => {
                        let retained = reply_text(&self.outgoing, id, live, status)?;
                        (retained.clone(), retained)
                    }
                };
                let work = Arc::new(Pending {
                    kind: BusyKind::Reply,
                    since: Instant::now(),
                    ..Pending::clone(&running.work)
                });
                self.unlist(&running.ended);
                let ended = self.list(&work);
                self.retained.insert(
                    id,
                    Retained {
                        text: retained,
                        work,
                        ended,
                    },
                );
                text
            }
            Some(running) => {
                self.unlist(&running.ended);
                reply_text(&self.outgoing, id, live, status)?
            }
            None => reply_text(&self.outgoing, id, live, status)?,
        };
        let _ = self.outgoing.push(text);
        Ok(())
    }

    fn resume(&mut self, pending: &[u64]) -> Result<(), CodeModeHostError> {
        if let Some(timer) = self.hold.take() {
            timer.abort();
        }
        for text in self.bus.take_held() {
            self.outgoing.push(text)?;
        }
        self.outgoing.flush()?;
        let wanted = pending.iter().copied().collect::<HashSet<_>>();
        let forgotten = self
            .retained
            .keys()
            .filter(|id| !wanted.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in forgotten {
            if let Some(retained) = self.retained.remove(&id) {
                self.unlist(&retained.ended);
            }
        }
        for id in pending {
            if let Some(retained) = self.retained.get(id) {
                self.outgoing.push(retained.text.clone())?;
            } else if !self.running.contains_key(id) {
                self.outgoing.send(&Frame::Reply {
                    id: *id,
                    result: Err(NOT_RECEIVED.into()),
                })?;
            }
        }
        Ok(())
    }

    fn end_turn(&mut self, id: u64, session_id: String, turn_id: String, hook_event_name: String) {
        self.turn_calls
            .remove(&(session_id.clone(), turn_id.clone()));
        let host = Arc::clone(&self.host);
        let finished = self.finished.clone();
        self.spawn(
            id,
            Box::pin(async move {
                host.end_turn(&session_id, &turn_id, &hook_event_name).await;
                let _ = finished.send(Done {
                    id,
                    live: Ok(None),
                    full: None,
                    status: None,
                });
            }),
        );
    }

    async fn frame(&mut self, frame: Frame) -> Result<(), CodeModeHostError> {
        match frame {
            Frame::CallbackResult { id, result } => self.bus.answer(id, result),
            Frame::Ack { ids } => {
                for id in ids {
                    if let Some(retained) = self.retained.remove(&id) {
                        self.unlist(&retained.ended);
                    }
                }
            }
            Frame::Resume { pending } => self.resume(&pending)?,
            Frame::Request {
                id,
                operation,
                max_frame_bytes,
                compact,
                output_prefix,
                resumable,
                answers,
                asks,
            } => {
                self.bus.answering.store(answers, Ordering::SeqCst);
                self.bus.asking.store(asks, Ordering::SeqCst);
                self.request(
                    id,
                    operation,
                    max_frame_bytes,
                    compact,
                    output_prefix,
                    resumable,
                    answers,
                )
                .await?;
            }
            _ => return Err(error("unexpected brain frame")),
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn request(
        &mut self,
        id: u64,
        mut operation: RemoteOperation,
        max_frame_bytes: Option<usize>,
        compact: bool,
        output_prefix: bool,
        resumable: bool,
        answers: bool,
    ) -> Result<(), CodeModeHostError> {
        let outgoing = self.outgoing.clone();
        if let Some(bytes) = max_frame_bytes {
            outgoing.limit.fetch_min(bytes, Ordering::Relaxed);
        }
        self.resumable |= resumable;
        let work = Arc::new(Pending::new(id, &operation));
        let known = self
            .running
            .get(&id)
            .map(|running| Arc::clone(&running.work))
            .or_else(|| {
                self.retained
                    .get(&id)
                    .map(|retained| Arc::clone(&retained.work))
            });
        if let Some(known) = known {
            if known.id != work.id {
                outgoing.send(&Frame::Reply {
                    id,
                    result: Err(REQUEST_IN_USE.into()),
                })?;
            } else if let Some(retained) = self.retained.get(&id) {
                outgoing.push(retained.text.clone())?;
            }
            return Ok(());
        }
        let session_id = work.session_id.clone();
        let ended = self.list(&work);
        self.running.insert(
            id,
            Running {
                resumable,
                connection: self.connection,
                work: Arc::clone(&work),
                ended,
                cancel: None,
            },
        );
        self.publish();
        if self.activity.draining()
            && matches!(
                &operation,
                RemoteOperation::Execute { .. } | RemoteOperation::Call { .. }
            )
        {
            self.finish(id, Err(NOT_RECEIVED.into()));
            return Ok(());
        }
        if let RemoteOperation::Execute {
            local_tools,
            local_tools_key: Some(key),
            ..
        } = &mut operation
        {
            if !local_tools.is_empty() {
                self.brain_tools.insert(key.clone(), local_tools.clone());
            } else if let Some(known) = self.brain_tools.get(key) {
                local_tools.clone_from(known);
            } else if let Some(known) = self.earlier_brain_tools.remove(key) {
                local_tools.clone_from(&known);
                self.brain_tools.insert(key.clone(), known);
            } else {
                self.finish(id, Err(LOCAL_TOOLS_UNKNOWN.into()));
                return Ok(());
            }
        }
        if let RemoteOperation::EndTurn {
            session_id,
            turn_id,
            hook_event_name,
        } = operation
        {
            self.end_turn(id, session_id, turn_id, hook_event_name);
            return Ok(());
        }
        let selected = match &operation {
            RemoteOperation::Execute { selected_tools, .. }
            | RemoteOperation::Call { selected_tools, .. } => Some(selected_tools.clone()),
            _ => None,
        };
        if let Some(Some(_)) = &selected
            && !self.host.selects_tools()
        {
            self.finish(
                id,
                Err("hand tool selection is unavailable with dynamic hand providers; nothing was dispatched".into()),
            );
            return Ok(());
        }
        if let Some(selected_tools) = &selected
            && !self.sessions.contains_key(&session_id)
        {
            let catalog = CallbackCatalog {
                session_id: Arc::from(session_id.as_str()),
                state: Arc::new(Atom::new(CallbackCatalogState {
                    selected_tools: selected_tools.clone(),
                    ..Default::default()
                })),
                bus: Arc::clone(&self.bus),
            };
            let brain = HandBrain {
                catalog: catalog.clone(),
                hands: Arc::clone(&self.callbacks),
            };
            let excluded = self
                .definitions()
                .await
                .into_iter()
                .filter(|definition| {
                    selected_tools
                        .as_ref()
                        .is_some_and(|names| !names.iter().any(|name| name == definition.name()))
                })
                .map(|definition| definition.name().to_owned())
                .collect();
            match self.host.open(&session_id, excluded, brain) {
                Ok(session) => {
                    self.sessions.insert(session_id.clone(), (session, catalog));
                    self.publish_sessions();
                }
                Err(issue) => {
                    self.finish(id, Err(issue.to_string()));
                    return Ok(());
                }
            }
        }
        if let Some(selected_tools) = &selected
            && let Some((_, catalog)) = self.sessions.get(&session_id)
            && catalog
                .state
                .read(|state| &state.selected_tools != selected_tools)
        {
            self.finish(
                id,
                Err("hand tool selection cannot change within a session".into()),
            );
            return Ok(());
        }
        if matches!(
            &operation,
            RemoteOperation::Cancel { .. } | RemoteOperation::CancelTurn { turn_id: None, .. }
        ) {
            if let Some(calls) = self.calls.remove(&session_id) {
                calls.cancel();
            }
            self.turn_calls
                .retain(|(session, _), _| session != &session_id);
        }
        if let RemoteOperation::CancelTurn {
            turn_id: Some(turn_id),
            ..
        } = &operation
        {
            if let Some(calls) = self
                .turn_calls
                .remove(&(session_id.clone(), turn_id.clone()))
            {
                calls.cancel();
            }
            self.finish(id, Ok(None));
            return Ok(());
        }
        if let RemoteOperation::CancelCall { call_id, .. } = &operation {
            for running in self.running.values().filter(|running| {
                running.work.session_id == session_id && running.work.id == *call_id
            }) {
                if let Some(cancel) = &running.cancel {
                    cancel.cancel();
                }
            }
            self.finish(id, Ok(None));
            return Ok(());
        }
        let session_calls = self.calls.entry(session_id.clone()).or_default().clone();
        let cancelled = match &operation {
            RemoteOperation::Call {
                context:
                    RemoteContext {
                        turn_id: Some(turn_id),
                        ..
                    },
                ..
            } => self
                .turn_calls
                .entry((session_id.clone(), turn_id.clone()))
                .or_insert_with(|| session_calls.child())
                .clone(),
            _ => session_calls,
        };
        let cancelled = match &operation {
            RemoteOperation::Call { .. } => {
                let call = cancelled.child();
                if let Some(running) = self.running.get_mut(&id) {
                    running.cancel = Some(call.clone());
                }
                call
            }
            _ => cancelled,
        };
        if let RemoteOperation::Execute {
            hand: Some(hand), ..
        } = &operation
        {
            self.callbacks.name.set(Some(hand.clone()));
        }
        if let RemoteOperation::Execute {
            local_tools,
            context,
            ..
        } = &operation
            && let Some((_, catalog)) = self.sessions.get(&session_id)
            && let Err(issue) = catalog.register(&context.call_id, &work.label, local_tools.clone())
        {
            self.finish(id, Err(issue.to_string()));
            return Ok(());
        }
        let session = if matches!(&operation, RemoteOperation::Cancel { .. }) {
            let removed = self.sessions.remove(&session_id);
            self.publish_sessions();
            removed
        } else {
            self.sessions
                .get(&session_id)
                .map(|(session, catalog)| (Arc::clone(session), catalog.clone()))
        };
        let session = session.map(|(session, catalog)| {
            let turn = catalog.turn();
            (session, catalog, turn)
        });
        if let Some((session, catalog, turn)) = &session {
            match &operation {
                RemoteOperation::CancelTurn { .. } => {
                    session
                        .cancel_turn(&mut Observer {
                            id,
                            outgoing: outgoing.clone(),
                            catalog: catalog.clone(),
                            calls: SentCalls::new(compact),
                        })
                        .await;
                    catalog.finish_turn(Some(*turn));
                    self.finish(id, Ok(None));
                    return Ok(());
                }
                RemoteOperation::BeginTurn { .. } => {
                    session.begin_turn();
                    catalog.begin_turn();
                    self.finish(id, Ok(None));
                    return Ok(());
                }
                RemoteOperation::SettlePreempt { .. } => {
                    session.settle_preempt();
                    self.finish(id, Ok(None));
                    return Ok(());
                }
                RemoteOperation::DetachTurn { session_id } => {
                    session
                        .detach_turn(&mut |origin| {
                            Box::new(DetachedObserver {
                                session_id: session_id.clone(),
                                origin_call_id: origin.into(),
                                outgoing: outgoing.clone(),
                                catalog: catalog.clone(),
                                calls: SentCalls::new(compact),
                            })
                        })
                        .await;
                    self.finish(id, Ok(None));
                    return Ok(());
                }
                _ => {}
            }
        }
        let finished = self.finished.clone();
        self.spawn(
            id,
            Box::pin(async move {
                let mut status = None;
                let mut full = None;
                let result = run(
                    id,
                    operation,
                    session,
                    &outgoing,
                    compact,
                    output_prefix,
                    resumable,
                    answers,
                    &cancelled,
                    &mut status,
                    &mut full,
                )
                .await
                .map_err(|issue| issue.to_string());
                let _ = finished.send(Done {
                    id,
                    live: result,
                    full,
                    status,
                });
            }),
        );
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    id: u64,
    operation: RemoteOperation,
    session: Option<(Arc<dyn HandSession>, CallbackCatalog, u64)>,
    outgoing: &Outgoing,
    compact: bool,
    output_prefix: bool,
    resumable: bool,
    answers: bool,
    cancelled: &Cancel,
    status: &mut Option<String>,
    full: &mut Option<Value>,
) -> Result<Option<Value>, CodeModeHostError> {
    let Some((session, catalog, turn)) = session else {
        return match operation {
            RemoteOperation::Execute { .. }
            | RemoteOperation::Wait { .. }
            | RemoteOperation::Call { .. } => Err(error(NO_CELLS)),
            _ => Ok(None),
        };
    };
    let answered = answers.then(|| Arc::clone(&catalog.bus.answers));
    let asks = catalog
        .bus
        .asking
        .load(Ordering::SeqCst)
        .then(|| Arc::clone(&catalog.bus.answers));
    let mut observer = Observer {
        id,
        outgoing: outgoing.clone(),
        catalog: catalog.clone(),
        calls: SentCalls::new(compact)
            .output_prefix(output_prefix)
            .answers(answered.clone())
            .asks(asks),
    };
    match operation {
        RemoteOperation::Execute {
            source, context, ..
        } => {
            let mut guard = OriginGuard {
                catalog: catalog.clone(),
                origin: context.call_id.clone(),
                replied: false,
            };
            let result = session
                .execute(&source, context.owned(), &mut observer)
                .await;
            if !result
                .as_ref()
                .is_ok_and(|execution| execution.cell.as_ref().is_some_and(|cell| cell.running))
            {
                catalog.forget(&guard.origin);
            }
            guard.replied = true;
            result.map(|value| {
                *status = Some(status_line(&value));
                *full = resumable.then(|| SentCalls::new(compact).encode(&value));
                Some(observer.calls.execution(value))
            })
        }
        RemoteOperation::Wait { input, context } => {
            let execution = session.wait(&input, context.owned(), &mut observer).await?;
            if let Some(cell) = &execution.cell {
                if cell.running {
                    catalog.adopt(&cell.origin_call_id, turn);
                } else {
                    catalog.forget(&cell.origin_call_id);
                }
            }
            *status = Some(status_line(&execution));
            *full = resumable.then(|| SentCalls::new(compact).encode(&execution));
            Ok(Some(observer.calls.execution(execution)))
        }
        RemoteOperation::Call {
            name,
            input,
            context,
            ..
        } => {
            tokio::select! {
                output = session.call(&name, input, context.owned()) => encode_output(output, compact, output_prefix).and_then(|value| {
                    if resumable && value.get("output_prefix").is_some() {
                        *full = Some(output_to_wire(output_from_wire(value.clone())?, compact)?);
                    }
                    Ok(Some(value))
                }),
                () = cancelled.cancelled() => Err(error(CALL_CANCELLED)),
            }
        }
        RemoteOperation::Cancel { .. } => {
            session.cancel(&mut observer).await;
            catalog.finish_turn(None);
            session.close().await;
            Ok(None)
        }
        RemoteOperation::CancelTurn { .. } => {
            session.cancel_turn(&mut observer).await;
            catalog.finish_turn(Some(turn));
            Ok(None)
        }
        RemoteOperation::DetachTurn { session_id } => {
            session
                .detach_turn(&mut |origin| {
                    Box::new(DetachedObserver {
                        session_id: session_id.clone(),
                        origin_call_id: origin.into(),
                        outgoing: outgoing.clone(),
                        catalog: catalog.clone(),
                        calls: SentCalls::new(compact),
                    })
                })
                .await;
            Ok(None)
        }
        RemoteOperation::BeginTurn { .. } => {
            session.begin_turn();
            catalog.begin_turn();
            Ok(None)
        }
        RemoteOperation::Preempt { .. } => {
            session.preempt().await;
            Ok(None)
        }
        RemoteOperation::SettlePreempt { .. } => {
            session.settle_preempt();
            Ok(None)
        }
        RemoteOperation::EndTurn { .. } | RemoteOperation::CancelCall { .. } => Ok(None),
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.activity.report(self.busy, false);
        self.activity.detach(&self.board);
    }
}
