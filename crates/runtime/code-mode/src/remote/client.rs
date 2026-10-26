use std::{
    collections::HashMap,
    future::{Future, poll_fn},
    pin::pin,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::Poll,
    time::Duration,
};

use nanocode_sync::{Atom, Slot};
use nanocode_tool_contracts::{ToolOutput, ToolReceipt};

use futures_util::{
    FutureExt, StreamExt,
    future::{AbortHandle, Abortable, Either, select},
    stream::FuturesUnordered,
};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::{
    HAND_RESTARTED, NOT_RECEIVED, REQUEST_IN_USE, ReceivedCalls, RemoteCodeModeHost, RemoteContext,
    RemoteFrame as Frame, RemoteOperation, RemoteTransport, encode_output,
};
use crate::{
    Tool, ToolDefinition, ToolInput,
    embedded::{CodeModeExecution, CodeModeHostError, CodeModeObserver, HandCalls, HostFuture},
};

pub const PEER_CLOSED: &str = "runner disconnected";
pub const LOCAL_CLOSED: &str = "remote connection closed";

pub type ResumedReply = HostFuture<'static, Result<Option<Value>, CodeModeHostError>>;

pub type CallReply = HostFuture<'static, Result<Value, CodeModeHostError>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteDelivery {
    Durable,
    Immediate,
}

pub trait RemoteLink: Send + Sync + 'static {
    fn supports_tool_selection(&self) -> bool {
        false
    }

    fn send(
        &self,
        frames: Vec<Frame>,
        delivery: RemoteDelivery,
    ) -> HostFuture<'_, Result<(), CodeModeHostError>>;
    fn receive(&self) -> HostFuture<'_, Result<Vec<Frame>, CodeModeHostError>>;
    fn close<'a>(&'a self, reason: &'a str) -> HostFuture<'a, ()>;
    fn changed(&self) {}
    fn max_frame_bytes(&self) -> Option<usize> {
        None
    }
    fn resumes(&self) -> Option<String> {
        None
    }
}

#[derive(Default)]
struct Observer {
    attached: AtomicBool,
    current: Slot<Box<dyn CodeModeObserver>>,
}

impl Observer {
    fn attach(&self, observer: Box<dyn CodeModeObserver>) {
        self.current.put(observer);
        self.attached.store(true, Ordering::SeqCst);
    }

    fn deliver(&self, update: Value) -> Result<(), CodeModeHostError> {
        let Some(mut observer) = self.current.take() else {
            return Ok(());
        };
        let delivered = ReceivedCalls::default().deliver(Some(observer.as_mut()), update);
        if let Some(newer) = self.current.put(observer) {
            self.current.put(newer);
        }
        delivered
    }
}

#[derive(Clone)]
struct CallbackState {
    tools: Vec<Arc<dyn Tool>>,
    context: RemoteContext,
    generation: u64,
    observer: Arc<Observer>,
}

type Callbacks = HashMap<(String, String), CallbackState>;

type Receipts = HashMap<(String, String), Vec<Weak<dyn ToolReceipt>>>;

type Sending<'a> = (HostFuture<'a, Result<(), CodeModeHostError>>, Vec<u64>);

#[derive(Clone)]
struct Waiter {
    replies: mpsc::UnboundedSender<Frame>,
    written: Arc<AtomicBool>,
}

type Waiters = HashMap<u64, Waiter>;

enum Outgoing {
    Send {
        frames: Vec<Frame>,
        delivery: RemoteDelivery,
        waiters: Vec<(u64, Waiter)>,
    },
    Forget(u64),
}

impl Outgoing {
    const fn frames(frames: Vec<Frame>, delivery: RemoteDelivery) -> Self {
        Self::Send {
            frames,
            delivery,
            waiters: Vec::new(),
        }
    }
}

fn waiter() -> (Waiter, mpsc::UnboundedReceiver<Frame>) {
    let (replies, receiver) = mpsc::unbounded_channel();
    (
        Waiter {
            replies,
            written: Arc::default(),
        },
        receiver,
    )
}

fn mark_written(pending: &Waiters, ids: &[u64]) {
    for id in ids {
        if let Some(waiter) = pending.get(id) {
            waiter.written.store(true, Ordering::SeqCst);
        }
    }
}

struct Client {
    outgoing: mpsc::UnboundedSender<Outgoing>,
    callbacks: Atom<Callbacks>,
    receipts: Atom<Receipts>,
    running: Atom<HashMap<u64, (String, AbortHandle)>>,
    next: AtomicU64,
    turns: Atom<HashMap<String, u64>>,
    operation_timeout: Option<Duration>,
    failure: Atom<Option<String>>,
    hands: Atom<Option<Arc<dyn HandCalls>>>,
    max_frame_bytes: Option<usize>,
    resumes: Option<String>,
    settles: bool,
    tool_selection: bool,
    _stop: oneshot::Sender<()>,
}

#[derive(Clone)]
pub struct RemoteClient {
    client: Arc<Client>,
}

#[derive(Clone)]
pub struct RemoteActivity(Weak<Client>);

fn error(value: impl std::fmt::Display) -> CodeModeHostError {
    CodeModeHostError::new(value.to_string())
}

fn replied(message: String) -> CodeModeHostError {
    if message == NOT_RECEIVED || message == REQUEST_IN_USE {
        CodeModeHostError::unsent(message)
    } else {
        CodeModeHostError::new(message)
    }
}

struct Resumed {
    client: Weak<Client>,
    id: u64,
}

impl Drop for Resumed {
    fn drop(&mut self) {
        if let Some(client) = self.client.upgrade() {
            let _ = client.outgoing.send(Outgoing::Forget(self.id));
        }
    }
}

fn resumed(
    client: Weak<Client>,
    id: u64,
    mut replies: mpsc::UnboundedReceiver<Frame>,
) -> ResumedReply {
    Box::pin(async move {
        let _resumed = Resumed { client, id };
        while let Some(frame) = replies.recv().await {
            match frame {
                Frame::Reply { result, .. } => {
                    let mut value = result.map_err(replied)?;
                    if let Some(value) = value.as_mut() {
                        ReceivedCalls::default().execution(value)?;
                    }
                    return Ok(value);
                }
                Frame::Update { .. } => {}
                _ => return Err(error("unexpected operation reply")),
            }
        }
        Err(error(
            "remote reply channel closed; operation outcome unknown",
        ))
    })
}

fn origin(call_id: &str) -> &str {
    call_id
        .rsplit_once("/code-")
        .map_or(call_id, |(origin, _)| origin)
}

fn fits(frame: &Frame, max_frame_bytes: Option<usize>) -> Result<(), CodeModeHostError> {
    max_frame_bytes.map_or(Ok(()), |limit| frame.encode(limit).map(drop))
}

fn requested_yield(operation: &RemoteOperation) -> Duration {
    let requested = match operation {
        RemoteOperation::Execute { source, .. } => source
            .lines()
            .next()
            .and_then(|line| line.trim_start().strip_prefix("// @exec:"))
            .and_then(|pragma| serde_json::from_str::<Value>(pragma).ok()),
        RemoteOperation::Wait { input, .. } => serde_json::from_str::<Value>(input).ok(),
        RemoteOperation::Call { input, .. } => Some(input.clone()),
        _ => None,
    };
    requested
        .and_then(|value| value.get("yield_time_ms")?.as_u64())
        .map_or(Duration::ZERO, Duration::from_millis)
}

impl RemoteActivity {
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.0.strong_count() > 0
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        self.0.upgrade().is_some_and(|client| {
            !client.running.read(HashMap::is_empty)
                || client.callbacks.read(|callbacks| {
                    callbacks
                        .values()
                        .any(|state| state.observer.attached.load(Ordering::SeqCst))
                })
        })
    }
}

impl RemoteClient {
    pub fn new(
        link: impl RemoteLink,
        operation_timeout: Option<Duration>,
    ) -> (Self, HostFuture<'static, String>) {
        let (client, driver, _) = Self::open(link, operation_timeout, Vec::new(), false);
        (client, driver)
    }

    pub fn resuming(
        link: impl RemoteLink,
        operation_timeout: Option<Duration>,
        pending: Vec<u64>,
    ) -> (Self, HostFuture<'static, String>, Vec<(u64, ResumedReply)>) {
        Self::open(link, operation_timeout, pending, true)
    }

    fn open(
        link: impl RemoteLink,
        operation_timeout: Option<Duration>,
        pending: Vec<u64>,
        settles: bool,
    ) -> (Self, HostFuture<'static, String>, Vec<(u64, ResumedReply)>) {
        let (outgoing, frames) = mpsc::unbounded_channel();
        let (stop, stopped) = oneshot::channel();
        let max_frame_bytes = link.max_frame_bytes();
        let resumes = link.resumes();
        let tool_selection = link.supports_tool_selection();
        let mut waiters = HashMap::new();
        let mut receivers = Vec::new();
        if resumes.is_some() {
            for id in &pending {
                let (replies, receiver) = mpsc::unbounded_channel();
                waiters.insert(
                    *id,
                    Waiter {
                        replies,
                        written: Arc::new(AtomicBool::new(true)),
                    },
                );
                receivers.push((*id, receiver));
            }
            let _ = outgoing.send(Outgoing::frames(
                vec![Frame::Resume { pending }],
                RemoteDelivery::Immediate,
            ));
        } else {
            for id in pending {
                let (replies, receiver) = mpsc::unbounded_channel();
                let _ = replies.send(Frame::Reply {
                    id,
                    result: Err(HAND_RESTARTED.to_owned()),
                });
                receivers.push((id, receiver));
            }
        }
        let client = Arc::new(Client {
            outgoing,
            callbacks: Atom::default(),
            receipts: Atom::default(),
            running: Atom::default(),
            next: AtomicU64::new(nanocode_platform::uuid_v4().as_u64_pair().0 >> 12),
            turns: Atom::default(),
            operation_timeout,
            failure: Atom::default(),
            hands: Atom::default(),
            max_frame_bytes,
            resumes,
            settles,
            tool_selection,
            _stop: stop,
        });
        let resumed = receivers
            .into_iter()
            .map(|(id, receiver)| (id, resumed(Arc::downgrade(&client), id, receiver)))
            .collect();
        let driver = Box::pin(drive(
            Arc::downgrade(&client),
            link,
            frames,
            stopped,
            waiters,
        ));
        (Self { client }, driver, resumed)
    }

    pub fn ack(&self, ids: Vec<u64>) -> Result<(), CodeModeHostError> {
        self.client
            .outgoing
            .send(Outgoing::frames(
                vec![Frame::Ack { ids }],
                RemoteDelivery::Immediate,
            ))
            .map_err(error)
    }

    #[must_use]
    pub fn activity(&self) -> RemoteActivity {
        RemoteActivity(Arc::downgrade(&self.client))
    }

    #[must_use]
    pub fn failure(&self) -> Option<String> {
        Option::clone(&self.client.failure.get())
    }

    #[must_use]
    pub fn into_host(self, definitions: Vec<ToolDefinition>) -> RemoteCodeModeHost {
        RemoteCodeModeHost::new(self, definitions)
    }

    pub fn calls(&self, calls: Vec<(u64, RemoteOperation)>) -> Vec<CallReply> {
        let mut frames = Vec::new();
        let mut waiters = Vec::new();
        let mut replies = Vec::new();
        for (id, operation) in calls {
            match prepare_call(&self.client, id, operation) {
                Ok((frame, waiter, reply)) => {
                    frames.push(frame);
                    waiters.push(waiter);
                    replies.push(reply);
                }
                Err(issue) => replies.push(Box::pin(async move { Err(issue) })),
            }
        }
        if !frames.is_empty()
            && self
                .client
                .outgoing
                .send(Outgoing::Send {
                    frames,
                    delivery: RemoteDelivery::Immediate,
                    waiters,
                })
                .is_err()
        {
            return replies
                .into_iter()
                .map(|reply| -> CallReply {
                    drop(reply);
                    Box::pin(async {
                        Err(CodeModeHostError::unsent(
                            "remote connection closed; the operation was not dispatched",
                        ))
                    })
                })
                .collect();
        }
        replies
    }
}

fn prepare_call(
    client: &Arc<Client>,
    id: u64,
    operation: RemoteOperation,
) -> Result<(Frame, (u64, Waiter), CallReply), CodeModeHostError> {
    client.not_dispatched()?;
    let requested = requested_yield(&operation);
    let frame = client.request_frame(id, operation);
    fits(&frame, client.max_frame_bytes).map_err(|issue| {
        CodeModeHostError::unsent(format!("{issue}; the operation was not dispatched"))
    })?;
    let (registered, mut replies) = waiter();
    let guard = Pending {
        client: Arc::clone(client),
        id,
        session_id: None,
        origin: None,
        complete: false,
    };
    let reply = Box::pin(async move {
        let reply = async {
            while let Some(frame) = replies.recv().await {
                match frame {
                    Frame::Reply { result, .. } => {
                        return result
                            .map_err(replied)?
                            .ok_or_else(|| error("hand omitted the tool result"));
                    }
                    Frame::Update { .. } => {}
                    _ => return Err(error("unexpected operation reply")),
                }
            }
            Err(error(
                "remote reply channel closed; operation outcome unknown",
            ))
        };
        guard
            .client
            .deadline(
                reply,
                requested,
                "remote operation deadline elapsed; outcome unknown; no retry",
            )
            .await?
    });
    Ok((frame, (id, registered), reply))
}

impl Client {
    fn dispatch(
        &self,
        frame: Frame,
        delivery: RemoteDelivery,
        waiter: Option<(u64, Waiter)>,
    ) -> Result<(), CodeModeHostError> {
        if let Frame::Request {
            operation: RemoteOperation::Cancel { session_id },
            ..
        } = &frame
        {
            self.callbacks
                .update(|callbacks| callbacks.retain(|(session, _), _| session != session_id));
            self.receipts
                .update(|receipts| receipts.retain(|(session, _), _| session != session_id));
            self.turns.update(|turns| {
                turns.remove(session_id);
            });
            let cancelled = self.running.update(|running| {
                let cancelled = running
                    .iter()
                    .filter(|(_, (session, _))| session == session_id)
                    .map(|(id, (_, abort))| (*id, abort.clone()))
                    .collect::<Vec<_>>();
                for (id, _) in &cancelled {
                    running.remove(id);
                }
                cancelled
            });
            for (_, abort) in cancelled {
                abort.abort();
            }
        }
        self.outgoing
            .send(Outgoing::Send {
                frames: vec![frame],
                delivery,
                waiters: waiter.into_iter().collect(),
            })
            .map_err(error)
    }

    fn receive(
        self: &Arc<Self>,
        frame: Frame,
        pending: &mut Waiters,
    ) -> Result<Option<HostFuture<'static, ()>>, CodeModeHostError> {
        match frame {
            Frame::Reply { id, .. } => {
                if let Some(waiter) = pending.remove(&id) {
                    let _ = waiter.replies.send(frame);
                }
            }
            Frame::Update { id, .. } => {
                if let Some(waiter) = pending.get(&id) {
                    let _ = waiter.replies.send(frame);
                }
            }
            Frame::Catalog {
                runtime_id,
                resumable,
                ..
            } => {
                if !resumable || runtime_id.is_none() || runtime_id != self.resumes {
                    return Err(error(HAND_RESTARTED));
                }
                let pending = pending
                    .iter()
                    .filter(|(_, waiter)| waiter.written.load(Ordering::SeqCst))
                    .map(|(id, _)| *id)
                    .collect();
                self.outgoing
                    .send(Outgoing::frames(
                        vec![Frame::Resume { pending }],
                        RemoteDelivery::Immediate,
                    ))
                    .map_err(error)?;
            }
            Frame::CallbackCancel { id } => {
                if let Some((_, abort)) = self.running.update(|running| running.remove(&id)) {
                    abort.abort();
                }
            }
            Frame::DetachedUpdate {
                session_id,
                origin_call_id,
                update,
            } => {
                let key = (session_id, origin_call_id);
                if let Some(observer) = self
                    .callbacks
                    .read(|callbacks| callbacks.get(&key).map(|state| Arc::clone(&state.observer)))
                {
                    observer.deliver(update)?;
                }
            }
            Frame::DetachedComplete {
                session_id,
                origin_call_id,
            } => {
                let key = (session_id, origin_call_id);
                self.callbacks.update(|callbacks| {
                    callbacks.remove(&key);
                });
            }
            Frame::Callback {
                id,
                name,
                input,
                freeform,
                context,
                hand,
                compact,
                output_prefix,
            } => {
                return Ok(Some(self.callback(
                    id,
                    name,
                    input,
                    freeform,
                    context,
                    hand,
                    compact,
                    output_prefix,
                )));
            }
            _ => return Err(error("unexpected runner frame")),
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    fn callback(
        self: &Arc<Self>,
        id: u64,
        name: String,
        input: Value,
        freeform: bool,
        context: RemoteContext,
        hand: Option<String>,
        compact: bool,
        output_prefix: bool,
    ) -> HostFuture<'static, ()> {
        let key = (
            context.session_id.clone(),
            origin(&context.call_id).to_owned(),
        );
        let local = self.callbacks.read(|callbacks| {
            callbacks.get(&key).map(|state| {
                (
                    state
                        .tools
                        .iter()
                        .find(|tool| tool.definition().name() == name)
                        .cloned(),
                    state.context.clone(),
                )
            })
        });
        let hands = Option::clone(&self.hands.get());
        let delivery = if hand.is_some() {
            RemoteDelivery::Immediate
        } else {
            RemoteDelivery::Durable
        };
        let (abort, registration) = AbortHandle::new_pair();
        self.running.update(|running| {
            running.insert(id, (context.session_id.clone(), abort.clone()));
        });
        let outgoing = self.outgoing.clone();
        let client = Arc::downgrade(self);
        let holder = Weak::clone(&client);
        let max_frame_bytes = self.max_frame_bytes;
        Box::pin(async move {
            let hold = |output: &mut ToolOutput| {
                if let Some(client) = holder.upgrade() {
                    client.hold_receipts(&key, output);
                }
            };
            let execution = Abortable::new(
                async move {
                    if let Some(hand) = hand {
                        let (_, mut authoritative) = local.ok_or_else(|| {
                            error(format!(
                                "brain callback origin for `{name}` on hand `{hand}` unavailable"
                            ))
                        })?;
                        let hands = hands.ok_or_else(|| {
                            error("this brain does not route calls between hands")
                        })?;
                        authoritative.call_id = context.call_id;
                        let owned = authoritative.owned();
                        let mut output = hands.call(&hand, &name, input, owned.as_context()).await;
                        hold(&mut output);
                        return encode_output(output, compact, output_prefix);
                    }
                    let Some((Some(tool), mut authoritative)) = local else {
                        return Err(error(format!("brain callback tool `{name}` unavailable")));
                    };
                    authoritative.call_id = context.call_id;
                    let owned = authoritative.owned();
                    let input = if freeform {
                        ToolInput::Freeform(
                            input
                                .as_str()
                                .ok_or_else(|| error("freeform callback must be a string"))?
                                .into(),
                        )
                    } else {
                        ToolInput::Function(serde_json::value::to_raw_value(&input).map_err(error)?)
                    };
                    let mut output = tool
                        .execute(input, owned.as_context())
                        .await
                        .map_err(error)?;
                    hold(&mut output);
                    encode_output(output, compact, output_prefix)
                },
                registration,
            );
            if let Ok(result) = execution.await {
                let mut frame = Frame::CallbackResult {
                    id,
                    result: result.map_err(|issue: CodeModeHostError| issue.to_string()),
                };
                if let Err(issue) = fits(&frame, max_frame_bytes) {
                    frame = Frame::CallbackResult {
                        id,
                        result: Err(format!("{issue}; the brain callback's result was not sent")),
                    };
                }
                let _ = outgoing.send(Outgoing::frames(vec![frame], delivery));
            }
            if let Some(client) = client.upgrade() {
                client.running.update(|running| {
                    running.remove(&id);
                });
            }
        })
    }

    fn terminate(&self, message: &str, pending: &mut Waiters) {
        for (_, (_, abort)) in self.running.update(std::mem::take) {
            abort.abort();
        }
        self.callbacks.set(HashMap::new());
        self.receipts.set(HashMap::new());
        let uncertain = format!(
            "{message}; the dispatched operation has an unknown outcome and was not retried"
        );
        let unsent = format!("{message}; the operation was not dispatched");
        self.failure.set(Some(message.to_owned()));
        for (id, waiter) in pending.drain() {
            let result = if waiter.written.load(Ordering::SeqCst) {
                uncertain.clone()
            } else {
                unsent.clone()
            };
            let _ = waiter.replies.send(Frame::Reply {
                id,
                result: Err(result),
            });
        }
    }

    fn refuse(ids: &[u64], message: &str, pending: &mut Waiters) {
        let refused = format!("{message}; the operation was not dispatched");
        for (id, waiter) in ids
            .iter()
            .filter_map(|id| pending.remove(id).map(|waiter| (*id, waiter)))
        {
            let _ = waiter.replies.send(Frame::Reply {
                id,
                result: Err(refused.clone()),
            });
        }
    }

    fn hold_receipts(&self, origin: &(String, String), output: &mut ToolOutput) {
        let held = output.take_receipts();
        if held.is_empty() {
            return;
        }
        for receipt in &held {
            receipt.carried_by(None);
        }
        self.receipts.update(|receipts| {
            receipts
                .retain(|_, receipts| receipts.iter().any(|receipt| receipt.strong_count() > 0));
            receipts
                .entry(origin.clone())
                .or_default()
                .extend(held.iter().map(Arc::downgrade));
        });
    }

    const fn request_frame(&self, id: u64, operation: RemoteOperation) -> Frame {
        Frame::Request {
            id,
            operation,
            max_frame_bytes: self.max_frame_bytes,
            compact: true,
            output_prefix: true,
            resumable: self.resumes.is_some(),
        }
    }

    fn not_dispatched(&self) -> Result<(), CodeModeHostError> {
        match Option::clone(&self.failure.get()) {
            Some(failure) => Err(error(format!(
                "{failure}; the operation was not dispatched"
            ))),
            None => Ok(()),
        }
    }

    async fn deadline<T>(
        &self,
        reply: impl Future<Output = T>,
        requested: Duration,
        elapsed: &str,
    ) -> Result<T, CodeModeHostError> {
        let Some(timeout) = self.operation_timeout else {
            return Ok(reply.await);
        };
        match select(
            pin!(reply),
            pin!(nanocode_platform::sleep(timeout.saturating_add(requested))),
        )
        .await
        {
            Either::Left((result, _)) => Ok(result),
            Either::Right(_) => Err(error(elapsed)),
        }
    }
}

async fn drive(
    client: Weak<Client>,
    link: impl RemoteLink,
    mut frames: mpsc::UnboundedReceiver<Outgoing>,
    mut stopped: oneshot::Receiver<()>,
    mut pending: Waiters,
) -> String {
    let mut callbacks = FuturesUnordered::new();
    let result = {
        let mut receiving = link.receive();
        let mut sending: Option<Sending<'_>> = None;
        let result = poll_fn(|cx| {
            loop {
                if stopped.poll_unpin(cx).is_ready() {
                    return Poll::Ready(Ok(()));
                }
                let mut progressed = false;
                if let Some((send, ids)) = sending.as_mut() {
                    match send.as_mut().poll(cx) {
                        Poll::Ready(Ok(())) => {
                            mark_written(&pending, ids);
                            sending = None;
                            progressed = true;
                        }
                        Poll::Ready(Err(issue))
                            if !issue.refused_requests().is_empty()
                                && issue.refused_requests().iter().all(|id| ids.contains(id)) =>
                        {
                            let refused = issue.refused_requests();
                            let sent: Vec<u64> = ids
                                .iter()
                                .copied()
                                .filter(|id| !refused.contains(id))
                                .collect();
                            mark_written(&pending, &sent);
                            Client::refuse(refused, issue.message(), &mut pending);
                            sending = None;
                            progressed = true;
                        }
                        Poll::Ready(Err(issue)) => {
                            if !issue.is_unsent() {
                                mark_written(&pending, ids);
                            }
                            sending = None;
                            return Poll::Ready(Err(issue));
                        }
                        Poll::Pending => {}
                    }
                }
                if sending.is_none() {
                    let mut batch = Vec::new();
                    let mut delivery = RemoteDelivery::Immediate;
                    while let Poll::Ready(next) = frames.poll_recv(cx) {
                        match next {
                            None => return Poll::Ready(Ok(())),
                            Some(Outgoing::Send {
                                frames,
                                delivery: needs,
                                waiters,
                            }) => {
                                pending.extend(waiters);
                                if needs == RemoteDelivery::Durable {
                                    delivery = needs;
                                }
                                batch.extend(frames);
                            }
                            Some(Outgoing::Forget(id)) => {
                                pending.remove(&id);
                            }
                        }
                    }
                    if !batch.is_empty() {
                        let ids = batch
                            .iter()
                            .filter_map(|frame| match frame {
                                Frame::Request { id, .. } => Some(*id),
                                _ => None,
                            })
                            .collect();
                        sending = Some((link.send(batch, delivery), ids));
                        continue;
                    }
                }
                match receiving.as_mut().poll(cx) {
                    Poll::Ready(Ok(received)) => {
                        let Some(client) = client.upgrade() else {
                            return Poll::Ready(Ok(()));
                        };
                        let mut acks = Vec::new();
                        for frame in received {
                            if client.resumes.is_some()
                                && !client.settles
                                && let Frame::Reply { id, .. } = &frame
                            {
                                acks.push(*id);
                            }
                            match client.receive(frame, &mut pending) {
                                Ok(Some(callback)) => callbacks.push(callback),
                                Ok(None) => {}
                                Err(issue) => return Poll::Ready(Err(issue)),
                            }
                        }
                        if !acks.is_empty() {
                            let _ = client.outgoing.send(Outgoing::frames(
                                vec![Frame::Ack { ids: acks }],
                                RemoteDelivery::Immediate,
                            ));
                        }
                        link.changed();
                        receiving = link.receive();
                        progressed = true;
                    }
                    Poll::Ready(Err(issue)) => return Poll::Ready(Err(issue)),
                    Poll::Pending => {}
                }
                while let Poll::Ready(Some(())) = callbacks.poll_next_unpin(cx) {
                    link.changed();
                    progressed = true;
                }
                if !progressed {
                    return Poll::Pending;
                }
            }
        })
        .await;
        if let Some((_, ids)) = sending.take() {
            mark_written(&pending, &ids);
        }
        result
    };
    drop(callbacks);
    frames.close();
    while let Ok(next) = frames.try_recv() {
        match next {
            Outgoing::Send { waiters, .. } => pending.extend(waiters),
            Outgoing::Forget(id) => {
                pending.remove(&id);
            }
        }
    }
    let message = result.map_or_else(|issue| issue.to_string(), |()| LOCAL_CLOSED.to_owned());
    if let Some(client) = client.upgrade() {
        client.terminate(&message, &mut pending);
    }
    link.close(&message).await;
    message
}

struct Pending {
    client: Arc<Client>,
    id: u64,
    session_id: Option<String>,
    origin: Option<(String, String)>,
    complete: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        let _ = self.client.outgoing.send(Outgoing::Forget(self.id));
        if !self.complete {
            if let Some(origin) = self.origin.take() {
                self.client.callbacks.update(|callbacks| {
                    callbacks.remove(&origin);
                });
            }
            if let Some(session_id) = self.session_id.take() {
                let id = self.client.next.fetch_add(1, Ordering::Relaxed);
                let _ = self.client.outgoing.send(Outgoing::frames(
                    vec![self.client.request_frame(
                        id,
                        RemoteOperation::CancelTurn {
                            session_id,
                            turn_id: None,
                        },
                    )],
                    RemoteDelivery::Durable,
                ));
            }
        }
    }
}

impl RemoteTransport for RemoteClient {
    fn supports_tool_selection(&self) -> bool {
        self.client.tool_selection
    }
    fn request<'a>(
        &'a self,
        operation: RemoteOperation,
        tools: Vec<Arc<dyn Tool>>,
        mut observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> HostFuture<'a, Result<Option<CodeModeExecution>, CodeModeHostError>> {
        Box::pin(async move {
            let client = &self.client;
            client.not_dispatched()?;
            let executed = match &operation {
                RemoteOperation::Execute { context, .. } => Some(context.clone()),
                _ => None,
            };
            let origin_call_id = executed.as_ref().map(|context| context.call_id.clone());
            let carrier = match &operation {
                RemoteOperation::Execute { context, .. }
                | RemoteOperation::Wait { context, .. } => Some(context.call_id.clone()),
                _ => None,
            };
            let session_id = match &operation {
                RemoteOperation::Execute { context, .. }
                | RemoteOperation::Wait { context, .. } => Some(context.session_id.clone()),
                _ => None,
            };
            let requested = requested_yield(&operation);
            let id = client.next.fetch_add(1, Ordering::Relaxed);
            let frame = client.request_frame(id, operation);
            fits(&frame, client.max_frame_bytes)
                .map_err(|issue| error(format!("{issue}; the operation was not dispatched")))?;
            if let Some(context) = executed {
                let generation = client
                    .turns
                    .read(|turns| turns.get(&context.session_id).copied())
                    .unwrap_or(0);
                let key = (context.session_id.clone(), context.call_id.clone());
                let state = CallbackState {
                    tools,
                    context,
                    generation,
                    observer: Arc::default(),
                };
                client.callbacks.modify(|callbacks| {
                    if callbacks.contains_key(&key) {
                        return Err(error("callback origin is already active"));
                    }
                    callbacks.insert(key.clone(), state.clone());
                    Ok(())
                })?;
            }
            let (registered, mut replies) = waiter();
            let mut guard = Pending {
                client: Arc::clone(client),
                id,
                session_id: session_id.clone(),
                origin: session_id.clone().zip(origin_call_id.clone()),
                complete: false,
            };
            client
                .dispatch(frame, RemoteDelivery::Durable, Some((id, registered)))
                .map_err(|_| error("remote connection closed; the operation was not dispatched"))?;
            let reply = async {
                let mut received = ReceivedCalls::default();
                while let Some(frame) = replies.recv().await {
                    match frame {
                        Frame::Reply { result, .. } => {
                            return result
                                .map_err(replied)?
                                .map(|mut value| {
                                    received.execution(&mut value)?;
                                    serde_json::from_value::<CodeModeExecution>(value)
                                        .map_err(error)
                                })
                                .transpose();
                        }
                        Frame::Update { update, .. } => {
                            received.deliver(observer.as_deref_mut(), update)?;
                        }
                        _ => return Err(error("unexpected operation reply")),
                    }
                }
                Err(error(
                    "remote reply channel closed; operation outcome unknown",
                ))
            };
            let result = client
                .deadline(
                    reply,
                    requested,
                    "remote operation deadline elapsed; outcome unknown; cancellation requested; no retry",
                )
                .await?;
            if let (Some(session), Ok(Some(execution))) = (&session_id, &result)
                && let Some(cell) = execution.cell.as_ref().filter(|cell| cell.running)
            {
                let generation = client
                    .turns
                    .read(|turns| turns.get(session).copied())
                    .unwrap_or(0);
                let key = (session.clone(), cell.origin_call_id.clone());
                client.callbacks.update(|callbacks| {
                    if let Some(state) = callbacks.get_mut(&key) {
                        state.generation = generation;
                    }
                });
            }
            if let Some(session_id) = session_id {
                let completed = match &result {
                    Ok(Some(execution)) => match &execution.cell {
                        Some(cell) if cell.running => None,
                        Some(cell) => Some(cell.origin_call_id.clone()),
                        None => origin_call_id,
                    },
                    Err(_) => origin_call_id,
                    Ok(None) => None,
                };
                if let Some(origin) = completed {
                    let key = (session_id, origin);
                    client.callbacks.update(|callbacks| {
                        callbacks.remove(&key);
                    });
                    let held = client
                        .receipts
                        .update(|receipts| receipts.remove(&key))
                        .unwrap_or_default();
                    if let (Ok(Some(_)), Some(carrier)) = (&result, &carrier) {
                        for receipt in held.iter().filter_map(Weak::upgrade) {
                            receipt.carried_by(Some(carrier));
                        }
                    }
                }
            }
            guard.complete = true;
            result
        })
    }

    fn call(&self, operation: RemoteOperation) -> HostFuture<'_, Result<Value, CodeModeHostError>> {
        Box::pin(async move {
            let client = &self.client;
            let id = client.next.fetch_add(1, Ordering::Relaxed);
            let (frame, registered, reply) = prepare_call(client, id, operation)?;
            client
                .dispatch(frame, RemoteDelivery::Immediate, Some(registered))
                .map_err(|_| error("remote connection closed; the operation was not dispatched"))?;
            reply.await
        })
    }

    fn route_hands(&self, hands: Arc<dyn HandCalls>) {
        self.client.hands.set(Some(hands));
    }

    fn notify(&self, operation: RemoteOperation) -> Result<(), CodeModeHostError> {
        if let RemoteOperation::BeginTurn { session_id } = &operation {
            self.client.turns.update(|turns| {
                *turns.entry(session_id.clone()).or_default() += 1;
            });
        }
        if let Some(failure) = Option::clone(&self.client.failure.get()) {
            return Err(error(failure));
        }
        self.client.dispatch(
            self.client
                .request_frame(self.client.next.fetch_add(1, Ordering::Relaxed), operation),
            RemoteDelivery::Durable,
            None,
        )
    }

    fn detach_turn(
        &self,
        session_id: &str,
        factory: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) -> Result<(), CodeModeHostError> {
        let generation = self
            .client
            .turns
            .read(|turns| turns.get(session_id).copied())
            .unwrap_or(0);
        let callbacks = self.client.callbacks.get();
        for ((session, origin), state) in callbacks.iter() {
            if session == session_id && state.generation == generation {
                state.observer.attach(factory(origin));
            }
        }
        self.notify(RemoteOperation::DetachTurn {
            session_id: session_id.into(),
        })
    }
}
