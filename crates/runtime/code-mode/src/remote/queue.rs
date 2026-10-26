use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::pin,
    sync::Arc,
    time::Duration,
};

use futures_util::{
    FutureExt, Stream, StreamExt,
    future::{BoxFuture, Either},
    stream::FuturesUnordered,
};
use serde_json::Value;
use tokio::{sync::mpsc, time::Instant};

use super::{
    NOT_RECEIVED, RemoteClient, RemoteContext, RemoteLink, RemoteOperation, RemoteTransport,
    output_from_wire,
};
use crate::embedded::CodeModeHostError;

const DERIVED_REQUESTS: u64 = 1 << 52;

#[must_use]
pub fn request_id(call_id: &str) -> u64 {
    let hash = call_id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    DERIVED_REQUESTS | ((hash ^ (hash >> 52)) & (DERIVED_REQUESTS - 1))
}

pub struct QueuedCall<O> {
    pub name: String,
    pub input: Value,
    pub context: RemoteContext,
    pub request: Option<u64>,
    pub origin: O,
}

impl<O> QueuedCall<O> {
    #[must_use]
    pub fn request_id(&self) -> u64 {
        self.request
            .unwrap_or_else(|| request_id(&self.context.call_id))
    }
}

pub struct Polled<O> {
    pub calls: Vec<QueuedCall<O>>,
    pub stop: Vec<(String, String)>,
    pub cancel: Vec<(String, String)>,
    pub watch: Vec<(String, String)>,
}

impl<O> Default for Polled<O> {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            stop: Vec::new(),
            cancel: Vec::new(),
            watch: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admit {
    Send,
    Wait,
    Refuse(String),
}

pub enum Settlement {
    Completed(Value),
    Unavailable(String),
    Unknown,
}

impl Settlement {
    #[must_use]
    pub fn outcome(&self) -> Option<Value> {
        match self {
            Self::Completed(output) => {
                Some(serde_json::json!({"status": "completed", "output": output}))
            }
            Self::Unavailable(message) => {
                Some(serde_json::json!({"status": "unavailable", "message": message}))
            }
            Self::Unknown => None,
        }
    }
}

pub trait CommandQueue: Send + Sync + 'static {
    type Origin: Clone + Send + Sync + 'static;
    type Error: std::fmt::Display + Send + 'static;

    fn poll(&self) -> impl Future<Output = Result<Polled<Self::Origin>, Self::Error>> + Send;
    fn admit(
        &self,
        calls: &[QueuedCall<Self::Origin>],
    ) -> impl Future<Output = Result<Vec<Admit>, Self::Error>> + Send {
        std::future::ready(Ok(vec![Admit::Send; calls.len()]))
    }
    fn settle(
        &self,
        settled: &[(String, Settlement)],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn turns_ended(&self, turns: &[(String, String)]) -> impl Future<Output = Vec<bool>> + Send;
    fn unsettled(&self, commands: &[String], error: &str);
}

#[derive(Clone, Copy)]
pub struct QueueOptions {
    pub operation_timeout: Option<Duration>,
    pub poll: Duration,
    pub busy_poll: Duration,
    pub end_check: Duration,
    pub cancels_calls: bool,
}

type Replied = BoxFuture<'static, (String, u64, bool, Result<Option<Value>, CodeModeHostError>)>;

enum Released {
    Settled(Vec<String>),
    Retry(Vec<String>),
    Refused(Vec<String>),
}

enum Done {
    Reply {
        command: String,
        request: u64,
        resumed: bool,
        stopped: Option<String>,
        result: Result<Option<Value>, CodeModeHostError>,
    },
    Refused {
        command: String,
        reason: String,
    },
}

pub async fn serve_queue<Q: CommandQueue>(
    link: impl RemoteLink,
    queue: Arc<Q>,
    wakeups: impl Stream<Item = ()> + Send,
    options: QueueOptions,
    failed: impl Future<Output = Q::Error>,
) -> Result<String, Q::Error> {
    let first = queue.poll().await?;
    let mut flights: HashMap<String, String> = HashMap::new();
    let mut stopped: HashMap<String, String> = HashMap::new();
    let pending = first.calls.iter().map(QueuedCall::request_id).collect();
    let (client, driver, resumed) =
        RemoteClient::resuming(link, options.operation_timeout, pending);
    let mut driver = tokio::spawn(driver);
    let mut replies: FuturesUnordered<Replied> = FuturesUnordered::new();
    for (call, (request, reply)) in first.calls.iter().zip(resumed) {
        let command = call.context.call_id.clone();
        flights.insert(command.clone(), call.context.session_id.clone());
        replies.push(Box::pin(
            reply.map(move |result| (command, request, true, result)),
        ));
    }
    let (done, settling) = mpsc::unbounded_channel();
    let (release, mut released) = mpsc::unbounded_channel();
    tokio::spawn(settle(
        Arc::clone(&queue),
        client.clone(),
        settling,
        release,
    ));
    let mut failed = pin!(failed);
    let mut wakeups = pin!(wakeups);
    let mut awake = true;
    let mut watching = HashSet::new();
    let mut cancelled = HashSet::new();
    let mut checked = Instant::now();
    let mut polled = Some(first);
    let mut next = Instant::now();
    let result = loop {
        if let Some(polled) = polled.take() {
            let calls = polled.calls.len();
            watching.extend(polled.watch);
            for (session_id, turn_id) in polled.cancel {
                if cancelled.insert((session_id.clone(), turn_id.clone())) {
                    notify(
                        &client,
                        RemoteOperation::CancelTurn {
                            session_id,
                            turn_id: Some(turn_id),
                        },
                    );
                }
            }
            for (command, reason) in polled.stop {
                if stopped.contains_key(&command) {
                    continue;
                }
                stopped.insert(command.clone(), reason.clone());
                match flights.get(&command) {
                    Some(session_id) if options.cancels_calls => notify(
                        &client,
                        RemoteOperation::CancelCall {
                            session_id: session_id.clone(),
                            call_id: command,
                        },
                    ),
                    Some(_) => {}
                    None => {
                        flights.insert(command.clone(), String::new());
                        let _ = done.send(Done::Refused { command, reason });
                    }
                }
            }
            let fresh = polled
                .calls
                .into_iter()
                .filter(|call| {
                    let command = &call.context.call_id;
                    !flights.contains_key(command) && !stopped.contains_key(command)
                })
                .collect::<Vec<_>>();
            if !fresh.is_empty() {
                let admitted = match queue.admit(&fresh).await {
                    Ok(admitted) => admitted,
                    Err(error) => break Err(error),
                };
                let mut operations = Vec::new();
                let mut sent = Vec::new();
                for (index, call) in fresh.into_iter().enumerate() {
                    let command = call.context.call_id.clone();
                    match admitted.get(index).cloned().unwrap_or(Admit::Wait) {
                        Admit::Send => {
                            let request = call.request_id();
                            flights.insert(command.clone(), call.context.session_id.clone());
                            sent.push((command, request));
                            operations.push((
                                request,
                                RemoteOperation::Call {
                                    name: call.name,
                                    input: call.input,
                                    selected_tools: None,
                                    context: call.context,
                                },
                            ));
                        }
                        Admit::Wait => {}
                        Admit::Refuse(reason) => {
                            flights.insert(command.clone(), String::new());
                            let _ = done.send(Done::Refused { command, reason });
                        }
                    }
                }
                for ((command, request), reply) in sent.into_iter().zip(client.calls(operations)) {
                    replies.push(Box::pin(
                        reply.map(move |result| (command, request, false, result.map(Some))),
                    ));
                }
            }
            if !watching.is_empty() && checked.elapsed() >= options.end_check {
                let turns = watching.iter().cloned().collect::<Vec<_>>();
                let ended = queue.turns_ended(&turns).await;
                for ((session_id, turn_id), ended) in turns.into_iter().zip(ended) {
                    if ended {
                        watching.remove(&(session_id.clone(), turn_id.clone()));
                        notify(
                            &client,
                            RemoteOperation::EndTurn {
                                session_id,
                                turn_id,
                                hook_event_name: "Stop".into(),
                            },
                        );
                    }
                }
                checked = Instant::now();
            }
            let busy = calls > 0 || !flights.is_empty() || !watching.is_empty();
            next = Instant::now()
                + if busy {
                    options.busy_poll
                } else {
                    options.poll
                };
        }
        let wakeup = if awake {
            Either::Left(wakeups.next())
        } else {
            Either::Right(std::future::pending())
        };
        tokio::select! {
            reason = &mut driver => break Ok(reason.unwrap_or_else(|issue| issue.to_string())),
            error = &mut failed => break Err(error),
            woken = wakeup => match woken {
                Some(()) => next = Instant::now(),
                None => awake = false,
            },
            Some(release) = released.recv() => {
                let commands = match release {
                    Released::Retry(commands) => {
                        next = Instant::now();
                        commands
                    }
                    Released::Settled(commands) | Released::Refused(commands) => commands,
                };
                for command in commands {
                    flights.remove(&command);
                    stopped.remove(&command);
                }
            },
            Some((command, request, resumed, result)) = replies.next(), if !replies.is_empty() => {
                let stopped = stopped.get(&command).cloned();
                let _ = done.send(Done::Reply { command, request, resumed, stopped, result });
            },
            () = tokio::time::sleep_until(next) => match queue.poll().await {
                Ok(next) => polled = Some(next),
                Err(error) => break Err(error),
            },
        }
    };
    driver.abort();
    result
}

fn notify(client: &RemoteClient, operation: RemoteOperation) {
    let client = client.clone();
    tokio::spawn(async move {
        drop(client.request(operation, Vec::new(), None).await);
    });
}

async fn settle<Q: CommandQueue>(
    queue: Arc<Q>,
    client: RemoteClient,
    mut done: mpsc::UnboundedReceiver<Done>,
    release: mpsc::UnboundedSender<Released>,
) {
    while let Some(first) = done.recv().await {
        let mut batch = vec![first];
        while let Ok(more) = done.try_recv() {
            batch.push(more);
        }
        let mut settled = Vec::new();
        let mut acks = Vec::new();
        let mut released = Vec::new();
        let mut refused = Vec::new();
        let mut finished = Vec::new();
        for item in batch {
            match item {
                Done::Refused { command, reason } => {
                    settled.push((command, Settlement::Unavailable(reason)));
                }
                Done::Reply {
                    command,
                    request,
                    resumed,
                    stopped,
                    result,
                } => match result {
                    Ok(Some(value)) => {
                        acks.push(request);
                        let completed = output_from_wire(value)
                            .and_then(|output| {
                                output
                                    .into_wire()
                                    .map_err(|issue| CodeModeHostError::new(issue.to_string()))
                            })
                            .and_then(|wire| {
                                serde_json::to_value(wire)
                                    .map_err(|issue| CodeModeHostError::new(issue.to_string()))
                            });
                        settled.push((
                            command,
                            completed.map_or(Settlement::Unknown, Settlement::Completed),
                        ));
                    }
                    Ok(None) => settled.push((command, Settlement::Unknown)),
                    Err(_) if client.failure().is_some() => released.push(command),
                    Err(error) if error.message() == NOT_RECEIVED => match stopped {
                        Some(reason) => settled.push((command, Settlement::Unavailable(reason))),
                        None if resumed => released.push(command),
                        None => refused.push(command),
                    },
                    Err(error) if error.is_unsent() => {
                        settled.push((command, Settlement::Unavailable(error.to_string())));
                    }
                    Err(_) => settled.push((command, Settlement::Unknown)),
                },
            }
        }
        if !settled.is_empty() {
            let commands = settled
                .iter()
                .map(|(command, _)| command.clone())
                .collect::<Vec<_>>();
            match queue.settle(&settled).await {
                Ok(()) => {
                    if !acks.is_empty() {
                        drop(client.ack(acks));
                    }
                    finished = commands;
                }
                Err(error) => {
                    queue.unsettled(&commands, &format!("{error:#}"));
                    released.extend(commands);
                }
            }
        }
        if !finished.is_empty() {
            let _ = release.send(Released::Settled(finished));
        }
        if !refused.is_empty() {
            let _ = release.send(Released::Refused(refused));
        }
        if !released.is_empty() {
            let _ = release.send(Released::Retry(released));
        }
    }
}
