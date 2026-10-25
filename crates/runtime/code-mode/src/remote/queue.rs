use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

use nanocode_sync::Atom;
use serde_json::Value;
use tokio::time::Instant;

use super::{
    RemoteClient, RemoteContext, RemoteDelivery, RemoteFrame, RemoteLink, RemoteOperation,
    RemoteTransport, output_from_wire,
};
use crate::embedded::{CodeModeHostError, HostFuture};

pub struct QueuedCall<O> {
    pub name: String,
    pub input: Value,
    pub context: RemoteContext,
    pub origin: O,
}

pub struct Polled<O> {
    pub calls: Vec<QueuedCall<O>>,
    pub cancel: Vec<(String, String)>,
    pub watch: Vec<(String, String)>,
}

impl<O> Default for Polled<O> {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            cancel: Vec::new(),
            watch: Vec::new(),
        }
    }
}

pub struct Recorded {
    pub queued: bool,
    pub request: Option<u64>,
}

pub enum Settlement {
    Completed(Value),
    Unavailable(String),
    Requeue,
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
            Self::Requeue | Self::Unknown => None,
        }
    }
}

pub trait CommandQueue: Send + Sync + 'static {
    type Origin: Clone + Send + Sync + 'static;
    type Error: std::fmt::Display + Send + 'static;

    fn poll(
        &self,
        in_flight: &HashSet<String>,
    ) -> impl Future<Output = Result<Polled<Self::Origin>, Self::Error>> + Send;
    fn admit(
        &self,
        calls: &[(u64, String, Self::Origin)],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn recorded(&self, command: &str)
    -> impl Future<Output = Result<Recorded, Self::Error>> + Send;
    fn settle(
        &self,
        command: &str,
        queued: bool,
        settlement: Settlement,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn turn_ended(&self, session_id: &str, turn_id: &str) -> impl Future<Output = bool> + Send;
    fn unsettled(&self, command: &str, error: &str);
}

#[derive(Clone, Copy)]
pub struct QueueOptions {
    pub operation_timeout: Option<Duration>,
    pub poll: Duration,
    pub end_check: Duration,
}

type Sending<O> = Arc<Atom<HashMap<String, O>>>;

struct QueueLink<L, Q: CommandQueue> {
    link: L,
    queue: Arc<Q>,
    sending: Sending<Q::Origin>,
}

impl<L: RemoteLink, Q: CommandQueue> RemoteLink for QueueLink<L, Q> {
    fn send(
        &self,
        frames: Vec<RemoteFrame>,
        delivery: RemoteDelivery,
    ) -> HostFuture<'_, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            let mut calls = Vec::new();
            for frame in &frames {
                match frame {
                    RemoteFrame::Request {
                        id,
                        operation: RemoteOperation::Call { context, .. },
                        ..
                    } => {
                        let origin = self
                            .sending
                            .read(|sending| sending.get(&context.call_id).cloned())
                            .ok_or_else(|| CodeModeHostError::unsent("unadmitted call"))?;
                        calls.push((*id, context.call_id.clone(), origin));
                    }
                    frame if frame.dispatches() => {
                        return Err(CodeModeHostError::unsent(
                            "a command queue sends only tool calls",
                        ));
                    }
                    _ => {}
                }
            }
            if !calls.is_empty() {
                self.queue
                    .admit(&calls)
                    .await
                    .map_err(|error| CodeModeHostError::unsent(format!("{error:#}")))?;
            }
            self.link.send(frames, delivery).await
        })
    }

    fn receive(&self) -> HostFuture<'_, Result<Vec<RemoteFrame>, CodeModeHostError>> {
        self.link.receive()
    }

    fn close<'a>(&'a self, reason: &'a str) -> HostFuture<'a, ()> {
        self.link.close(reason)
    }

    fn changed(&self) {
        self.link.changed();
    }

    fn max_frame_bytes(&self) -> Option<usize> {
        self.link.max_frame_bytes()
    }

    fn resumes(&self) -> Option<String> {
        self.link.resumes()
    }
}

pub async fn serve_queue<Q: CommandQueue>(
    link: impl RemoteLink,
    queue: Arc<Q>,
    sent: Vec<(u64, String)>,
    options: QueueOptions,
    failed: impl Future<Output = Q::Error>,
) -> Result<String, Q::Error> {
    let sending: Sending<Q::Origin> = Arc::default();
    let link = QueueLink {
        link,
        queue: Arc::clone(&queue),
        sending: Arc::clone(&sending),
    };
    let pending = sent.iter().map(|(request, _)| *request).collect();
    let (client, mut driver, resumed) =
        RemoteClient::resuming(link, options.operation_timeout, pending);
    let mut commands: HashMap<u64, String> = sent.into_iter().collect();
    for (request, reply) in resumed {
        if let Some(command) = commands.remove(&request) {
            let (queue, client) = (Arc::clone(&queue), client.clone());
            tokio::spawn(async move {
                settle(&*queue, &client, &command, reply.await).await;
            });
        }
    }
    let mut failed = std::pin::pin!(failed);
    let mut tick = tokio::time::interval(options.poll);
    let mut cancelled = HashSet::new();
    let mut watching = HashSet::new();
    let mut checked = Instant::now();
    loop {
        tokio::select! {
            reason = &mut driver => return Ok(reason),
            error = &mut failed => return Err(error),
            _ = tick.tick() => {
                let in_flight = sending.read(|sending| sending.keys().cloned().collect());
                let polled = queue.poll(&in_flight).await?;
                watching.extend(polled.watch);
                for (session_id, turn_id) in polled.cancel {
                    if cancelled.insert((session_id.clone(), turn_id.clone())) {
                        notify(&client, RemoteOperation::CancelTurn {
                            session_id,
                            turn_id: Some(turn_id),
                        });
                    }
                }
                for call in polled.calls {
                    dispatch(&queue, &client, &sending, call);
                }
                if !watching.is_empty() && checked.elapsed() >= options.end_check {
                    let mut ended = Vec::new();
                    for (session_id, turn_id) in &watching {
                        if queue.turn_ended(session_id, turn_id).await {
                            ended.push((session_id.clone(), turn_id.clone()));
                        }
                    }
                    for key in ended {
                        watching.remove(&key);
                        let (session_id, turn_id) = key;
                        notify(&client, RemoteOperation::EndTurn {
                            session_id,
                            turn_id,
                            hook_event_name: "Stop".into(),
                        });
                    }
                    checked = Instant::now();
                }
            }
        }
    }
}

fn notify(client: &RemoteClient, operation: RemoteOperation) {
    let client = client.clone();
    tokio::spawn(async move {
        drop(client.request(operation, Vec::new(), None).await);
    });
}

fn dispatch<Q: CommandQueue>(
    queue: &Arc<Q>,
    client: &RemoteClient,
    sending: &Sending<Q::Origin>,
    call: QueuedCall<Q::Origin>,
) {
    let command = call.context.call_id.clone();
    sending.update(|sending| {
        sending.insert(command.clone(), call.origin.clone());
    });
    let operation = RemoteOperation::Call {
        name: call.name,
        input: call.input,
        selected_tools: None,
        context: call.context,
    };
    let (queue, client, sending) = (Arc::clone(queue), client.clone(), Arc::clone(sending));
    tokio::spawn(async move {
        let result = client.call(operation).await.map(Some);
        settle(&*queue, &client, &command, result).await;
        sending.update(|sending| {
            sending.remove(&command);
        });
    });
}

async fn settle<Q: CommandQueue>(
    queue: &Q,
    client: &RemoteClient,
    command: &str,
    result: Result<Option<Value>, CodeModeHostError>,
) {
    if let Err(error) = try_settle(queue, client, command, result).await {
        queue.unsettled(command, &error);
    }
}

async fn try_settle<Q: CommandQueue>(
    queue: &Q,
    client: &RemoteClient,
    command: &str,
    result: Result<Option<Value>, CodeModeHostError>,
) -> Result<(), String> {
    let host = |error: Q::Error| format!("{error:#}");
    let recorded = queue.recorded(command).await.map_err(host)?;
    let settlement = match result {
        Ok(Some(value)) => {
            let output = output_from_wire(value).map_err(|error| error.to_string())?;
            let wire = output.into_wire().map_err(|error| error.to_string())?;
            Settlement::Completed(serde_json::to_value(wire).map_err(|error| error.to_string())?)
        }
        Err(error) if error.is_unsent() && !recorded.queued => Settlement::Requeue,
        Err(_) if client.failure().is_some() => return Ok(()),
        Err(error) if recorded.queued => Settlement::Unavailable(error.to_string()),
        _ => Settlement::Unknown,
    };
    let receipt = matches!(
        settlement,
        Settlement::Completed(_) | Settlement::Unavailable(_)
    );
    queue
        .settle(command, recorded.queued, settlement)
        .await
        .map_err(host)?;
    if receipt && let Some(request) = recorded.request {
        drop(client.ack(vec![request]));
    }
    Ok(())
}
