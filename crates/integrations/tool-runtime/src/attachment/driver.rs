use std::{
    borrow::Cow,
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nanocode_sync::Atom;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{
    client_async_tls_with_config,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};
use tracing::Instrument as _;
use url::Url;

use super::protocol::{
    self, AttachmentCall, CommandStatus, DiagnosticStage, EndpointFrame, ReceiptTiming, RunnerFrame,
};
use super::{
    AttachmentCallOutcome, AttachmentError, AttachmentEvent, AttachmentMetadata, AttachmentStatus,
};
use crate::prepared::{PreparedToolCall, PreparedToolError, PreparedToolRuntime};

pub(crate) const INITIAL_TRANSPORT_BACKOFF: Duration = Duration::from_millis(100);

pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn tls_connector<T>(
    request: &http::Request<T>,
) -> std::io::Result<Option<tokio_tungstenite::Connector>> {
    if request.uri().scheme_str() != Some("wss") {
        return Ok(None);
    }
    Ok(Some(tokio_tungstenite::Connector::Rustls(
        nanocode_tls::native_client_config().await?,
    )))
}

const PONG_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) const STABLE_CONNECTION: Duration = Duration::from_secs(30);

const HEARTBEAT_TIMEOUT_REASON: &str = "attachment heartbeat timed out";

pub(crate) use nanocode_tool_host::attachment::ATTACHMENT_REVOKED_REASON;

const MAX_REJECTED_BACKOFF: Duration = Duration::from_secs(60);

pub(crate) const MAX_TRANSPORT_BACKOFF: Duration = Duration::from_secs(5);

const MAX_SERVICE_BACKOFF: Duration = Duration::from_secs(2);

const FAST_SERVICE_WINDOW: Duration = Duration::from_secs(30);

pub(crate) struct Config {
    pub(crate) endpoint: Url,
    pub(crate) listener: Option<tokio::net::TcpListener>,
    pub(crate) authorization: Box<str>,
    pub(crate) tools: Value,
    pub(crate) metadata: Option<AttachmentMetadata>,
}

pub(crate) enum Command {
    Detach,
}

pub(crate) async fn run(
    config: Config,
    runtime: Arc<PreparedToolRuntime>,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<AttachmentEvent>,
    status: watch::Sender<AttachmentStatus>,
    closed: watch::Sender<Option<Result<(), AttachmentError>>>,
) {
    let runtime_id = uuid::Uuid::new_v4().to_string();
    let mut active = Vec::<InFlight>::new();
    let mut journal = HashMap::<Box<str>, RetainedCall>::new();
    let (completed_tx, mut completed_rx) = mpsc::unbounded_channel::<Completion>();
    let mut backoff = INITIAL_TRANSPORT_BACKOFF;
    let mut attempt = 0_u64;
    let mut previous_delay = Duration::ZERO;

    let mut service_outage: Option<Instant> = None;
    let terminal = loop {
        attempt = attempt.saturating_add(1);
        let connection_id = uuid::Uuid::new_v4().to_string();
        let connection_span = tracing::info_span!(target: "nanocode_code_mode::attachment", "attachment.connection",
            runtime_id = runtime_id.as_str(), client_connection_id = connection_id.as_str(),
            attempt, reconnect_delay_ms = previous_delay.as_millis() as u64);
        let _ = status.send(AttachmentStatus::Connecting);
        connection_span.in_scope(|| emit(&events, AttachmentEvent::Connecting));
        let request = match request(&config, &connection_id, &runtime_id) {
            Ok(request) => request,
            Err(error) => break Err(error),
        };
        let connect_started = Instant::now();
        let connected = if let Some(listener) = &config.listener {
            tokio::select! {
                _ = commands.recv() => break Ok(()),
                accepted = accept_runner(listener, &config.authorization, &events) => Ok(accepted),
            }
        } else {
            tokio::select! {
                command = commands.recv() => match command { Some(Command::Detach) | None => break Ok(()) },
                connected = tokio::time::timeout(Duration::from_secs(5), async {
                    let connector = tls_connector(&request).await?;
                    tracing::info!(target: "nanocode_code_mode::attachment",
                        stage = "attachment.socket.trust",
                        elapsed_ms = connect_started.elapsed().as_secs_f64() * 1000.0,
                        "attachment TLS trust ready");
                    let host = request.uri().host().ok_or(tokio_tungstenite::tungstenite::Error::Url(
                        tokio_tungstenite::tungstenite::error::UrlError::NoHostName,
                    ))?;
                    let host = host.trim_start_matches('[').trim_end_matches(']');
                    let port = request.uri().port_u16().unwrap_or(if connector.is_some() { 443 } else { 80 });
                    let dns_started = Instant::now();
                    let addresses: Vec<_> = tokio::net::lookup_host((host, port)).await?.collect();
                    tracing::info!(target: "nanocode_code_mode::attachment",
                        stage = "attachment.socket.resolved",
                        duration_ms = dns_started.elapsed().as_secs_f64() * 1000.0,
                        elapsed_ms = connect_started.elapsed().as_secs_f64() * 1000.0,
                        "attachment address resolved");
                    let tcp_started = Instant::now();
                    let stream = tokio::net::TcpStream::connect(addresses.as_slice()).await?;
                    stream.set_nodelay(true)?;
                    tracing::info!(target: "nanocode_code_mode::attachment",
                        stage = "attachment.socket.tcp",
                        duration_ms = tcp_started.elapsed().as_secs_f64() * 1000.0,
                        elapsed_ms = connect_started.elapsed().as_secs_f64() * 1000.0,
                        "attachment TCP connected");
                    let upgrade_started = Instant::now();
                    let upgraded = client_async_tls_with_config(request, stream, None, connector).await;

                    tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.socket.tls_upgrade",
                        duration_ms = upgrade_started.elapsed().as_secs_f64() * 1000.0,
                        reason_code = if upgraded.is_ok() { "connected" } else { "tls_upgrade_failure" }, "attachment TLS and upgrade finished");
                    upgraded
                }.instrument(connection_span.clone())) => connected,
            }
        };
        let socket = match connected {
            Ok(Ok((socket, response))) => {
                tracing::info!(target: "nanocode_code_mode::attachment",
                    stage = "attachment.websocket_connected",
                    duration_ms = connect_started.elapsed().as_secs_f64() * 1000.0,
                    runtime_id = runtime_id.as_str(), client_connection_id = connection_id.as_str(), attempt,
                    request_id = response.headers().get("x-nanocode-request-id").and_then(|v| v.to_str().ok()).and_then(safe_uuid).unwrap_or_default(),
                    "attachment WebSocket connected");
                socket
            }
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response)))
                if matches!(response.status().as_u16(), 401 | 403) =>
            {
                connection_span.in_scope(|| tracing::warn!(target: "nanocode_code_mode::attachment", stage = "attachment.socket.rejected", reason_code = "authentication_rejected", pending_calls = active.len(), "attachment connection rejected"));
                break Err(AttachmentError::Authentication(
                    "endpoint rejected the bearer credential".into(),
                ));
            }
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response)))
                if response.status().as_u16() == 410 =>
            {
                connection_span.in_scope(|| tracing::warn!(target: "nanocode_code_mode::attachment", stage = "attachment.socket.rejected", reason_code = "attachment_revoked", pending_calls = active.len(), "attachment permanently revoked"));
                break Err(AttachmentError::Fenced(ATTACHMENT_REVOKED_REASON.into()));
            }
            other => {
                let (failure, http_status) = match &other {
                    Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
                        let code = response.status().as_u16();
                        let retry_after = retry_after(response.headers());
                        let failure = if (500..=599).contains(&code) {
                            Failure::Service { retry_after }
                        } else {
                            Failure::Rejected { retry_after }
                        };
                        (failure, Some(code))
                    }
                    _ => (Failure::Transport, None),
                };
                let outage_ms = if matches!(failure, Failure::Service { .. }) {
                    let started = *service_outage.get_or_insert_with(Instant::now);
                    Some(started.elapsed().as_millis() as u64)
                } else {
                    service_outage = None;
                    None
                };
                let delay = failure.delay(&mut backoff, service_outage);
                let reason_code = match failure {
                    Failure::Service { .. } => "service_unavailable",
                    Failure::Rejected { .. } => "http_rejected",
                    Failure::Transport if other.is_err() => "connect_timeout",
                    Failure::Transport => "connect_failure",
                };
                let retry_after_ms = match failure {
                    Failure::Service {
                        retry_after: Some(after),
                    }
                    | Failure::Rejected {
                        retry_after: Some(after),
                    } => Some(after.as_millis() as u64),
                    _ => None,
                };
                connection_span.in_scope(|| tracing::warn!(target: "nanocode_code_mode::attachment", stage = "attachment.socket.connect_failed", reason_code, http_status, retry_after_ms, service_outage_ms = outage_ms, reconnect_delay_ms = delay.as_millis() as u64, pending_calls = active.len(), "attachment connection attempt failed"));
                let _ = status.send(AttachmentStatus::Disconnected);
                emit(
                    &events,
                    AttachmentEvent::Retrying {
                        reason: reason_code.into(),
                        delay_ms: delay.as_millis() as u64,
                    },
                );
                previous_delay = delay;
                if wait_backoff(&mut commands, delay).await {
                    break Ok(());
                }
                continue;
            }
        };
        service_outage = None;
        let connected_at = Instant::now();
        let end = connection(
            socket,
            ConnectionContext {
                config: &config,
                runtime_id: &runtime_id,
                connection_id: &connection_id,
                runtime: &runtime,
                events: &events,
                status: &status,
                active: &mut active,
                journal: &mut journal,
                completed_tx: &completed_tx,
                completed_rx: &mut completed_rx,
            },
            &mut commands,
        )
        .instrument(connection_span.clone())
        .await;
        if matches!(*status.borrow(), AttachmentStatus::Ready)
            && connected_at.elapsed() >= STABLE_CONNECTION
        {
            backoff = INITIAL_TRANSPORT_BACKOFF;
        }

        let delay = matches!(
            end,
            ConnectionEnd::Failed(_)
                | ConnectionEnd::Disconnected
                | ConnectionEnd::HeartbeatTimeout
        )
        .then(|| Failure::Transport.delay(&mut backoff, None));
        connection_span.in_scope(|| {
            if let ConnectionEnd::Failed(error) | ConnectionEnd::DetachFailed(error) = &end {
                tracing::debug!(target: "nanocode_code_mode::attachment", %error, "attachment transport ended");
            }
            tracing::info!(target: "nanocode_code_mode::attachment",
            stage = "attachment.socket.closed", reason_code = end.reason_code(),
            pending_calls = active.iter().filter(|call| !call.task.is_finished()).count(),
            reconnect_delay_ms = delay.map(|delay| delay.as_millis() as u64), "attachment connection ended")
        });
        match end {
            ConnectionEnd::Detached => break Ok(()),
            ConnectionEnd::DetachFailed(error) => break Err(error),
            ConnectionEnd::Rejected(reason) => {
                let _ = status.send(AttachmentStatus::Fenced);
                emit(
                    &events,
                    AttachmentEvent::Fenced {
                        reason: reason.clone(),
                    },
                );
                break Err(AttachmentError::Fenced(reason));
            }
            ConnectionEnd::Failed(_)
            | ConnectionEnd::Disconnected
            | ConnectionEnd::HeartbeatTimeout => {
                let _ = status.send(AttachmentStatus::Disconnected);
                let delay = delay.unwrap_or(MAX_TRANSPORT_BACKOFF);
                emit(
                    &events,
                    AttachmentEvent::Retrying {
                        reason: end.reason_code().into(),
                        delay_ms: delay.as_millis() as u64,
                    },
                );
                previous_delay = delay;
                if wait_backoff(&mut commands, delay).await {
                    break Ok(());
                }
            }
        }
    };

    let _ = status.send(
        if matches!(
            terminal,
            Err(AttachmentError::Authentication(_) | AttachmentError::Fenced(_))
        ) {
            AttachmentStatus::Fenced
        } else {
            AttachmentStatus::Disconnected
        },
    );
    shutdown_calls(&mut active).await;
    runtime.shutdown().await;
    if terminal.is_ok() {
        emit(
            &events,
            AttachmentEvent::Detached {
                reason: "closed".into(),
            },
        );
    }
    let _ = closed.send(Some(terminal));
}

fn request(
    config: &Config,
    connection_id: &str,
    runtime_id: &str,
) -> Result<http::Request<()>, AttachmentError> {
    let mut request = config
        .endpoint
        .as_str()
        .into_client_request()
        .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
    let mut authorization = http::HeaderValue::from_str(&config.authorization)
        .map_err(|_| AttachmentError::Authentication("invalid bearer credential".into()))?;
    authorization.set_sensitive(true);
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, authorization);
    request.headers_mut().insert(
        "x-nanocode-request-id",
        http::HeaderValue::from_str(connection_id)
            .map_err(|_| AttachmentError::Transport("invalid connection identity".into()))?,
    );

    if config.endpoint.path() == "/v1/account/tool-host"
        && let Some(machine) = config
            .metadata
            .as_ref()
            .and_then(AttachmentMetadata::attached_machine)
    {
        request.headers_mut().insert(
            "x-nanocode-hand-machine-id",
            http::HeaderValue::from_str(machine.id())
                .map_err(|_| AttachmentError::Transport("invalid machine identity".into()))?,
        );
        request.headers_mut().insert(
            "x-nanocode-hand-runtime-id",
            http::HeaderValue::from_str(runtime_id)
                .map_err(|_| AttachmentError::Transport("invalid runtime identity".into()))?,
        );
    }
    Ok(request)
}

fn safe_uuid(value: &str) -> Option<String> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| id.get_version_num() == 4 && id.get_variant() == uuid::Variant::RFC4122)
        .map(|id| id.to_string())
}

#[derive(Clone, Copy)]
pub(crate) enum Failure {
    Transport,

    Service { retry_after: Option<Duration> },

    Rejected { retry_after: Option<Duration> },
}

impl Failure {
    pub(crate) fn delay(self, backoff: &mut Duration, service_outage: Option<Instant>) -> Duration {
        let cap = match self {
            Self::Transport => MAX_TRANSPORT_BACKOFF,
            Self::Service { .. }
                if service_outage
                    .is_some_and(|started| started.elapsed() < FAST_SERVICE_WINDOW) =>
            {
                MAX_SERVICE_BACKOFF
            }
            Self::Service { .. } => MAX_TRANSPORT_BACKOFF,
            Self::Rejected { .. } => MAX_REJECTED_BACKOFF,
        };
        let scheduled = (*backoff).min(cap);
        *backoff = (scheduled * 2).min(cap);
        match self {
            Self::Service {
                retry_after: Some(after),
            }
            | Self::Rejected {
                retry_after: Some(after),
            } => scheduled.max(after),
            _ => scheduled,
        }
    }
}

fn retry_after(headers: &http::HeaderMap) -> Option<Duration> {
    let value = headers
        .get(http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(
        at.duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

async fn wait_backoff(commands: &mut mpsc::Receiver<Command>, delay: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        command = commands.recv() => matches!(command, Some(Command::Detach) | None),
    }
}

enum ConnectionEnd {
    Detached,
    DetachFailed(AttachmentError),
    Disconnected,
    HeartbeatTimeout,
    Failed(AttachmentError),
    Rejected(Box<str>),
}

impl ConnectionEnd {
    const fn reason_code(&self) -> &'static str {
        match self {
            Self::Detached => "detached",
            Self::DetachFailed(_) => "detach_transport_failure",
            Self::Disconnected => "remote_disconnect",
            Self::HeartbeatTimeout => "heartbeat_timeout",
            Self::Failed(_) => "transport_failure",
            Self::Rejected(_) => "protocol_rejected",
        }
    }
}

enum Completion {
    Diagnostic {
        call_id: Box<str>,
        timing: Arc<Atom<CallClock>>,
    },
    Result {
        call_id: Box<str>,
        outcome: Value,
    },
}

#[derive(Clone)]
struct CallClock {
    received: Instant,
    task_started: Option<Instant>,
    execution_started: Option<Instant>,
    execution_finished: Option<Instant>,
    result_queued: Option<Instant>,
    diagnostics_sent: u8,
    span: tracing::Span,
}

impl CallClock {
    fn received() -> Arc<Atom<Self>> {
        Arc::new(Atom::new(Self {
            received: Instant::now(),
            task_started: None,
            execution_started: None,
            execution_finished: None,
            result_queued: None,
            diagnostics_sent: 0,
            span: tracing::Span::current(),
        }))
    }

    fn snapshot(&self, now: Instant) -> ReceiptTiming {
        let started = self
            .execution_started
            .unwrap_or(self.task_started.unwrap_or(now));
        let finished = self.execution_finished.unwrap_or(now);
        let queued = self.result_queued.unwrap_or(now);
        let ms = |end: Instant, start: Instant| {
            end.saturating_duration_since(start).as_secs_f64() * 1000.0
        };
        ReceiptTiming {
            scheduler_ms: ms(started, self.received),
            execution_gate_ms: 0.0,
            execution_ms: if self.execution_started.is_some() {
                ms(finished, started)
            } else {
                0.0
            },
            result_encode_ms: if self.execution_finished.is_some() {
                ms(queued, finished)
            } else {
                0.0
            },
            result_queue_ms: if self.result_queued.is_some() {
                ms(now, queued)
            } else {
                0.0
            },
            host_elapsed_ms: ms(now, self.received),
        }
    }
}

struct RetainedCall {
    identity: CallIdentity,
    task: Option<tokio::task::AbortHandle>,
    timing: Arc<Atom<CallClock>>,
    receipt: Option<String>,
}

impl RetainedCall {
    fn finish(&mut self, outcome: &Value) -> Result<(), AttachmentError> {
        if self.receipt.is_none() {
            self.receipt = Some(encode_result(
                &self.identity.call_id,
                outcome,
                &self.timing,
            )?);
            self.task = None;
            let span = self.timing.read(|clock| clock.span.clone());
            span.in_scope(|| {
                tracing::info!(target: "nanocode_code_mode::attachment",
                stage = "attachment.result_retained",
                transport_call_id = self.identity.call_id.as_ref(),
                "attachment terminal receipt retained")
            });
        }
        Ok(())
    }
}

struct InFlight {
    task: tokio::task::JoinHandle<()>,
}

async fn shutdown_calls(active: &mut Vec<InFlight>) {
    for call in active.iter() {
        call.task.abort();
    }
    for call in active.drain(..) {
        let _ = call.task.await;
    }
}

struct TaskEvents {
    call: Option<CallEvents>,
    events: mpsc::Sender<AttachmentEvent>,
}

impl Drop for TaskEvents {
    fn drop(&mut self) {
        if let Some(call) = self.call.take() {
            call.complete(&self.events, AttachmentCallOutcome::Ambiguous);
        }
    }
}

struct CallEvents {
    call_id: Box<str>,
    span: tracing::Span,
    started_at: Instant,
}

impl CallEvents {
    fn complete(self, events: &mpsc::Sender<AttachmentEvent>, outcome: AttachmentCallOutcome) {
        let status = attachment_call_outcome_name(outcome);
        let duration_ns = u64::try_from(self.started_at.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.span.record("status", status);
        self.span.record(
            "otel.status_code",
            if matches!(outcome, AttachmentCallOutcome::Completed) {
                "OK"
            } else {
                "ERROR"
            },
        );
        self.span.record("duration_ns", duration_ns);
        self.span.in_scope(|| {
            record_attachment_call_completion(outcome);
        });
        emit(
            events,
            AttachmentEvent::CallCompleted {
                call_id: self.call_id,
                outcome,
            },
        );
    }
}

fn begin_call_events(
    events: &mpsc::Sender<AttachmentEvent>,
    call_id: Box<str>,
    name: Box<str>,
    attachment_id: Option<&str>,
) -> CallEvents {
    let span = tracing::info_span!(
        target: "nanocode_code_mode::attachment",
        "attachment.call",
        otel.kind = "server",
        otel.status_code = tracing::field::Empty,
        attachment.id = tracing::field::Empty,
        tool.name = name.as_ref(),
        tool.call_id = call_id.as_ref(),
        status = tracing::field::Empty,
        duration_ns = tracing::field::Empty,
    );
    if let Some(attachment_id) = attachment_id {
        span.record("attachment.id", attachment_id);
    }
    span.in_scope(|| {
        tracing::info!(
            target: "nanocode_code_mode::attachment",
            stage = "attachment.call.started",
            "attachment call started"
        );
    });
    emit(
        events,
        AttachmentEvent::CallStarted {
            call_id: call_id.clone(),
            name,
        },
    );
    CallEvents {
        call_id,
        span,
        started_at: Instant::now(),
    }
}

fn record_attachment_call_completion(outcome: AttachmentCallOutcome) {
    if matches!(
        outcome,
        AttachmentCallOutcome::Unavailable | AttachmentCallOutcome::Ambiguous
    ) {
        tracing::warn!(
            target: "nanocode_code_mode::attachment",
            stage = "attachment.call.completed",
            "attachment call completed"
        );
    } else {
        tracing::info!(
            target: "nanocode_code_mode::attachment",
            stage = "attachment.call.completed",
            "attachment call completed"
        );
    }
}

const fn attachment_call_outcome_name(outcome: AttachmentCallOutcome) -> &'static str {
    outcome.as_str()
}

#[derive(Clone, PartialEq)]
struct CallIdentity {
    session_id: Box<str>,
    turn_id: Option<Box<str>>,
    call_id: Box<str>,
    model: Box<str>,
    name: Box<str>,
    input: Value,
    output_token_budget: u64,
    output_byte_budget: u64,
    deadline_at: u64,
}

#[allow(
    clippy::too_many_arguments,
    reason = "keep per-call identity and independently owned execution/event channels explicit"
)]
fn start_call(
    runtime: &Arc<PreparedToolRuntime>,
    active: &mut Vec<InFlight>,
    identity: CallIdentity,
    timing: Arc<Atom<CallClock>>,
    tool_timeout: u64,
    events: CallEvents,
    completed: mpsc::UnboundedSender<Completion>,
    event_sender: &mpsc::Sender<AttachmentEvent>,
) -> tokio::task::AbortHandle {
    let runtime = Arc::clone(runtime);
    let task_span = events.span.clone();
    let mut events = TaskEvents {
        call: Some(events),
        events: event_sender.clone(),
    };
    let task = tokio::spawn(
        async move {
            let started = Instant::now();
            timing.update(|clock| clock.task_started = Some(started));
            let task_identity = identity;
            let remaining = task_identity.deadline_at.saturating_sub(now_ms());
            let (outcome, observed) = if remaining == 0 {
                (
                    unavailable("tool deadline elapsed before execution"),
                    AttachmentCallOutcome::Unavailable,
                )
            } else {
                let duration = Duration::from_millis(remaining.min(tool_timeout));
                let call = PreparedToolCall::new(
                    task_identity.model.to_string(),
                    task_identity.session_id.to_string(),
                    task_identity.call_id.to_string(),
                    task_identity.name.to_string(),
                    task_identity.input.clone(),
                    if task_identity.output_token_budget == protocol::JSON_SAFE_INTEGER_MAX {
                        crate::contract::DEFAULT_TOOL_OUTPUT_TOKENS
                    } else {
                        task_identity.output_token_budget as usize
                    },
                )
                .with_turn_id(task_identity.turn_id.as_deref().map(str::to_owned));
                let execute = async {
                    let _phase = ExecutionPhase::start(&timing, &completed, &task_identity.call_id);
                    runtime.execute(call).await
                };
                let executed = tokio::time::timeout(duration, execute).await;
                match executed {
                    Ok(Ok(output)) => match serde_json::to_value(output) {
                        Ok(output)
                            if serde_json::to_vec(&output).is_ok_and(|bytes| {
                                bytes.len() as u64 <= task_identity.output_byte_budget
                            }) =>
                        {
                            (
                                json!({"status":"completed", "output":output}),
                                AttachmentCallOutcome::Completed,
                            )
                        }
                        Ok(_) => bounded_completed_failure(
                            "tool output exceeded byte budget",
                            task_identity.output_byte_budget,
                        ),
                        Err(_) => (
                            ambiguous("tool output could not be encoded"),
                            AttachmentCallOutcome::Ambiguous,
                        ),
                    },
                    Ok(Err(error @ PreparedToolError::InvalidOutput(_))) => (
                        ambiguous(&error.to_string()),
                        AttachmentCallOutcome::Ambiguous,
                    ),
                    Ok(Err(error)) => (
                        unavailable(&error.to_string()),
                        AttachmentCallOutcome::Unavailable,
                    ),
                    Err(_) => (
                        ambiguous("tool deadline elapsed"),
                        AttachmentCallOutcome::Ambiguous,
                    ),
                }
            };
            if let Some(call) = events.call.take() {
                call.complete(&events.events, observed);
            }

            let queued = Instant::now();
            timing.update(|clock| clock.result_queued = Some(queued));
            let _ = completed.send(Completion::Result {
                call_id: task_identity.call_id,
                outcome,
            });
        }
        .instrument(task_span),
    );
    let abort = task.abort_handle();
    active.push(InFlight { task });
    abort
}

struct ExecutionPhase<'a> {
    timing: &'a Arc<Atom<CallClock>>,
    completed: &'a mpsc::UnboundedSender<Completion>,
    call_id: &'a str,
}

impl<'a> ExecutionPhase<'a> {
    fn start(
        timing: &'a Arc<Atom<CallClock>>,
        completed: &'a mpsc::UnboundedSender<Completion>,
        call_id: &'a str,
    ) -> Self {
        let started = Instant::now();
        timing.update(|clock| clock.execution_started = Some(started));
        let phase = Self {
            timing,
            completed,
            call_id,
        };
        phase.trace(DiagnosticStage::ExecutionStarted);
        phase.notify();
        phase
    }

    fn trace(&self, stage: DiagnosticStage) {
        let clock = self.timing.get();
        let elapsed_ms =
            (clock.received.elapsed().as_secs_f64() * 1000.0).min(9_007_199_254_740_991.0);
        clock.span.in_scope(|| tracing::info!(target: "nanocode_code_mode::attachment", stage = stage.name(), elapsed_ms, "attachment execution phase"));
    }

    fn notify(&self) {
        let _ = self.completed.send(Completion::Diagnostic {
            call_id: self.call_id.into(),
            timing: self.timing.clone(),
        });
    }
}

impl Drop for ExecutionPhase<'_> {
    fn drop(&mut self) {
        let finished = Instant::now();
        self.timing
            .update(|clock| clock.execution_finished = Some(finished));
        self.trace(DiagnosticStage::ExecutionFinished);
        self.notify();
    }
}

struct ConnectionContext<'a> {
    config: &'a Config,
    runtime_id: &'a str,
    connection_id: &'a str,
    runtime: &'a Arc<PreparedToolRuntime>,
    events: &'a mpsc::Sender<AttachmentEvent>,
    status: &'a watch::Sender<AttachmentStatus>,
    active: &'a mut Vec<InFlight>,
    journal: &'a mut HashMap<Box<str>, RetainedCall>,
    completed_tx: &'a mpsc::UnboundedSender<Completion>,
    completed_rx: &'a mut mpsc::UnboundedReceiver<Completion>,
}

async fn connection<S>(
    mut socket: tokio_tungstenite::WebSocketStream<S>,
    context: ConnectionContext<'_>,
    commands: &mut mpsc::Receiver<Command>,
) -> ConnectionEnd
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let ConnectionContext {
        config,
        runtime_id,
        connection_id,
        runtime,
        events,
        status,
        active,
        journal,
        completed_tx,
        completed_rx,
    } = context;
    let catalog_started = Instant::now();
    if let Err(error) = send(
        &mut socket,
        &RunnerFrame::Catalog {
            capabilities: vec!["turn_metadata".into()],
            runtime_id: runtime_id.into(),
            command_recovery: true,
            turn_lifecycle: true,
            diagnostics: Some(true),
            connection_id: Some(connection_id.into()),
            tools: Cow::Borrowed(&config.tools),
            machines: config
                .metadata
                .as_ref()
                .and_then(AttachmentMetadata::attached_machine)
                .map(|machine| Cow::Borrowed(std::slice::from_ref(machine))),
            attachment_id: config
                .metadata
                .as_ref()
                .map(|metadata| metadata.attachment_id().into()),
        },
    )
    .await
    {
        return ConnectionEnd::Failed(error);
    }
    let catalog_sent = Instant::now();
    tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.catalog_sent", duration_ms = catalog_sent.duration_since(catalog_started).as_secs_f64() * 1000.0, "attachment catalog sent");
    match next_handshake_frame(&mut socket, commands).await {
        Ok(EndpointFrame::Ready {}) => {}
        Ok(frame) => {
            return reject(
                &mut socket,
                format!("expected ready after catalog, received {}", frame.kind()),
            )
            .await;
        }
        Err(ConnectionEnd::Rejected(reason)) => return reject(&mut socket, reason).await,
        Err(end) => return end,
    }
    tracing::info!(target: "nanocode_code_mode::attachment",
        stage = "attachment.catalog_ready",
        send_ms = catalog_sent.duration_since(catalog_started).as_secs_f64() * 1000.0,
        acknowledge_ms = catalog_sent.elapsed().as_secs_f64() * 1000.0,
        "attachment catalog acknowledged");
    emit(events, AttachmentEvent::Attached);
    let _ = status.send(AttachmentStatus::Ready);
    emit(
        events,
        AttachmentEvent::CatalogPublished {
            tool_count: config.tools.as_array().map_or(0, Vec::len),
        },
    );

    while let Ok(completion) = completed_rx.try_recv() {
        if let Completion::Result {
            call_id, outcome, ..
        } = completion
            && let Some(call) = journal.get_mut(&call_id)
            && let Err(error) = call.finish(&outcome)
        {
            return ConnectionEnd::Failed(error);
        }
    }
    for call in journal.values() {
        let replayed = true;
        if call.receipt.is_some()
            && let Err(error) = send_retained(&mut socket, call, replayed).await
        {
            return ConnectionEnd::Failed(error);
        }
    }
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + protocol::HEARTBEAT_INTERVAL,
        protocol::HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pong_timeout = Box::pin(tokio::time::sleep(PONG_TIMEOUT));
    let mut awaiting_pong: Option<Vec<u8>> = None;
    let mut ping_started: Option<Instant> = None;
    let mut detach_deadline = Box::pin(tokio::time::sleep(Duration::from_secs(10)));
    let mut detaching = false;
    let mut draining = false;

    let end = loop {
        if detaching && draining && journal.is_empty() {
            break ConnectionEnd::Detached;
        }
        tokio::select! {
            command = commands.recv(), if !detaching => {
                match command { Some(Command::Detach) | None => {} }
                if let Err(error) = send(&mut socket, &RunnerFrame::Drain {}).await {
                    break ConnectionEnd::DetachFailed(error);
                }
                tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.drain_requested", pending_calls = journal.values().filter(|call| call.task.is_some()).count(), pending_receipts = journal.values().filter(|call| call.receipt.is_some()).count(), reason_code = "detach_requested", "attachment drain requested");
                detaching = true;
                detach_deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(10));
                shutdown_calls(active).await;
                while let Ok(completion) = completed_rx.try_recv() {
                    if let Completion::Result { call_id, outcome, .. } = completion
                        && let Some(call) = journal.get_mut(&call_id)
                        && let Err(error) = call.finish(&outcome) { return ConnectionEnd::DetachFailed(error); }
                }
                for call in journal.values_mut() {
                    let replayed = call.receipt.is_some();
                    if call.receipt.is_none()
                        && let Err(error) = call.finish(&ambiguous("attachment shut down during execution")) { return ConnectionEnd::DetachFailed(error); }
                    if let Err(error) = send_retained(&mut socket, call, replayed).await { return ConnectionEnd::DetachFailed(error); }
                }
            }
            _ = &mut detach_deadline, if detaching => break ConnectionEnd::Detached,
            _ = &mut pong_timeout, if awaiting_pong.is_some() => {
                tracing::warn!(target: "nanocode_code_mode::attachment", stage = "attachment.heartbeat_timeout", pending_calls = journal.values().filter(|call| call.task.is_some()).count(), pending_receipts = journal.values().filter(|call| call.receipt.is_some()).count(), reason_code = "heartbeat_timeout", "attachment heartbeat timed out");
                break ConnectionEnd::HeartbeatTimeout;
            },
            _ = heartbeat.tick() => {
                if awaiting_pong.is_some() { break ConnectionEnd::HeartbeatTimeout; }
                let nonce = uuid::Uuid::new_v4().as_bytes().to_vec();
                ping_started = Some(Instant::now());
                if let Err(error) = socket.send(Message::Ping(nonce.clone().into())).await.map_err(|error| AttachmentError::Transport(error.to_string().into())) {
                    break if detaching { ConnectionEnd::DetachFailed(error) } else { ConnectionEnd::Failed(error) };
                }
                tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.heartbeat_ping", pending_calls = journal.values().filter(|call| call.task.is_some()).count(), pending_receipts = journal.values().filter(|call| call.receipt.is_some()).count(), "attachment heartbeat sent");
                awaiting_pong = Some(nonce);
                pong_timeout.as_mut().reset(tokio::time::Instant::now() + PONG_TIMEOUT);
            }
            completion = completed_rx.recv() => if let Some(completion) = completion {
                let sent = match completion {
                    Completion::Diagnostic { call_id, timing } => {
                        if !journal.contains_key(&call_id) { continue; }
                        send_diagnostics(&mut socket, &call_id, &timing, false).await
                    }
                    Completion::Result { call_id, outcome } => {
                        let Some(call) = journal.get_mut(&call_id) else { continue; };
                        let replayed = call.receipt.is_some();
                        if let Err(error) = call.finish(&outcome) { break ConnectionEnd::Failed(error); }
                        send_retained(&mut socket, call, replayed).await
                    }
                };
                if let Err(error) = sent {
                    break if detaching { ConnectionEnd::DetachFailed(error) } else { ConnectionEnd::Failed(error) };
                }
            },
            incoming = socket.next() => {
                if let Some(Ok(Message::Pong(payload))) = &incoming {
                    if awaiting_pong.as_deref() == Some(payload.as_ref()) {
                        awaiting_pong = None;
                        let roundtrip_ms = ping_started.take().expect("pending ping clock").elapsed().as_secs_f64() * 1000.0;
                        tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.transport_rtt", connection_id, roundtrip_ms, "established socket heartbeat roundtrip observed");
                        tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.heartbeat_pong", pending_calls = journal.values().filter(|call| call.task.is_some()).count(), pending_receipts = journal.values().filter(|call| call.receipt.is_some()).count(), "attachment heartbeat acknowledged");
                    }
                    continue;
                }
                let frame = match incoming_frame(&mut socket, incoming).await {
                    Ok(Some(frame)) => frame,
                    Ok(None) => continue,
                    Err(ConnectionEnd::Disconnected) if detaching => break ConnectionEnd::DetachFailed(AttachmentError::Transport("websocket closed while draining".into())),
                    Err(end) => break end,
                };
                match frame {
                    EndpointFrame::Call(AttachmentCall { session_id, turn_id, call_id, model, name, input, output_token_budget, output_byte_budget, deadline_at }) => {
                        let timing = CallClock::received();
                        if draining { break ConnectionEnd::Rejected("call received after drain barrier".into()); }
                        let identity = CallIdentity { session_id:session_id.clone().into(), turn_id:turn_id.clone().map(Into::into), call_id:call_id.clone().into(), model:model.clone().into(), name:name.clone().into(), input:input.clone(), output_token_budget, output_byte_budget, deadline_at };
                        if let Some(retained) = journal.get(call_id.as_str()) {
                            if retained.identity != identity { break ConnectionEnd::Rejected("call identity conflicts with retained command".into()); }
                            let sent = if retained.receipt.is_some() {
                                send_retained(&mut socket, retained, true).await
                            } else { send(&mut socket, &RunnerFrame::Status { call_id: call_id.as_str().into(), state: CommandStatus::Running }).await };
                            if let Err(error) = sent { break ConnectionEnd::Failed(error); }
                            continue;
                        }
                        let phase_span = tracing::info_span!(target: "nanocode_code_mode::attachment", "attachment.call.phases",
                            transport_call_id = call_id.as_str(), session_id = session_id.as_str(),
                            host_turn_id = turn_id.as_deref().filter(|id| id.len() <= 256 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))).unwrap_or(""));
                        timing.update(|clock| clock.span = phase_span.clone());
                        if let Err(error) = send_diagnostics(&mut socket, &call_id, &timing, false).await { break ConnectionEnd::Failed(error); }
                        let call_events = begin_call_events(
                            events,
                            call_id.clone().into(),
                            name.clone().into(),
                            config.metadata.as_ref().map(AttachmentMetadata::attachment_id),
                        );
                        let tool_timeout = runtime.timeout_ms(&name).unwrap_or(0);
                        active.retain(|call| !call.task.is_finished());
                        let reason = if tool_timeout == 0 {
                            Some("tool is not in the pinned catalog")
                        } else if deadline_at <= now_ms() {
                            Some("tool deadline elapsed before execution")
                        } else { None };
                        if let Some(reason) = reason {
                            call_events.complete(events, AttachmentCallOutcome::Unavailable);
                            let mut retained = RetainedCall { identity, task: None, timing, receipt: None };
                            if let Err(error) = retained.finish(&unavailable(reason)) { break ConnectionEnd::Failed(error); }
                            journal.insert(call_id.clone().into(), retained);
                            if let Err(error) = send_retained(&mut socket, journal.get(call_id.as_str()).unwrap(), false).await { break ConnectionEnd::Failed(error); }
                            continue;
                        }
                        let task = start_call(runtime, active, identity.clone(), Arc::clone(&timing), tool_timeout, call_events, completed_tx.clone(), events);
                        journal.insert(call_id.into(), RetainedCall { identity, task: Some(task), timing, receipt: None });
                    }
                    EndpointFrame::TurnEnded { session_id, turn_id, hook_event_name } => {

                        if tokio::time::timeout(Duration::from_secs(5), runtime.end_turn(&session_id, &turn_id, &hook_event_name)).await.is_err() {
                            tracing::warn!(target: "nanocode_code_mode::attachment",
                                "turn cleanup timed out; not retried");
                        }
                    }
                    EndpointFrame::Cancel { call_id } => {
                        tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.call.cancel_received", transport_call_id = call_id.as_str(), pending_calls = journal.values().filter(|call| call.task.is_some()).count(), pending_receipts = journal.values().filter(|call| call.receipt.is_some()).count(), reason_code = "cancel_received", "attachment cancellation received");
                        if let Some(call) = journal.get_mut(call_id.as_str()) {
                            let replayed = call.receipt.is_some();

                            if call.timing.read(|clock| clock.result_queued.is_some()) { continue; }
                            if let Some(task) = call.task.take() {
                                task.abort();
                                if let Some(index) = active.iter().position(|active| active.task.id() == task.id()) {
                                    let _ = active.swap_remove(index).task.await;
                                }

                                if call.timing.read(|clock| clock.result_queued.is_some()) { continue; }
                                if let Err(error) = call.finish(&ambiguous("tool execution was cancelled after dispatch")) { break ConnectionEnd::Failed(error); }
                            }
                            if call.receipt.is_some()
                                && let Err(error) = send_retained(&mut socket, call, replayed).await { break ConnectionEnd::Failed(error); }
                        }
                    }
                    EndpointFrame::Ack { call_id } => {
                        tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.result_ack_received", transport_call_id = call_id.as_str(), pending_calls = journal.values().filter(|call| call.task.is_some()).count(), pending_receipts = journal.values().filter(|call| call.receipt.is_some()).count(), "attachment receipt acknowledgement received");
                        if journal.get(call_id.as_str()).is_some_and(|call| call.receipt.is_some()) {
                            journal.remove(call_id.as_str());
                        }
                    }
                    EndpointFrame::Recover { call_ids } => {
                        for call_id in call_ids {
                            let sent = match journal.get(call_id.as_str()) {
                                Some(call) if call.receipt.is_some() => send_retained(&mut socket, call, true).await,
                                Some(_) => send(&mut socket, &RunnerFrame::Status { call_id: call_id.as_str().into(), state: CommandStatus::Running }).await,
                                None => send(&mut socket, &RunnerFrame::Status { call_id: call_id.as_str().into(), state: CommandStatus::Missing }).await,
                            };
                            if let Err(error) = sent { return ConnectionEnd::Failed(error); }
                        }
                    }
                    EndpointFrame::Draining {} => {
                        if !detaching || draining { break ConnectionEnd::Rejected("unexpected draining acknowledgement".into()); }
                        tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.draining", pending_calls = journal.values().filter(|call| call.task.is_some()).count(), pending_receipts = journal.values().filter(|call| call.receipt.is_some()).count(), "attachment drain barrier acknowledged");
                        draining = true;
                    }
                    EndpointFrame::Ready {} => break ConnectionEnd::Rejected("unexpected ready".into()),
                }
            }
        }
    };

    tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.socket.pending_at_close", reason_code = end.reason_code(), pending_calls = journal.values().filter(|call| call.task.is_some()).count(), pending_receipts = journal.values().filter(|call| call.receipt.is_some()).count(), "attachment socket pending work at close");
    if matches!(&end, ConnectionEnd::HeartbeatTimeout) {
        close_heartbeat_timeout(&mut socket).await;
    }

    let end = if detaching {
        match end {
            ConnectionEnd::Disconnected => ConnectionEnd::DetachFailed(AttachmentError::Transport(
                "websocket disconnected while draining".into(),
            )),
            ConnectionEnd::HeartbeatTimeout => ConnectionEnd::DetachFailed(
                AttachmentError::Transport(HEARTBEAT_TIMEOUT_REASON.into()),
            ),
            ConnectionEnd::Failed(error) => ConnectionEnd::DetachFailed(error),
            end => end,
        }
    } else {
        end
    };

    match &end {
        ConnectionEnd::Detached => {
            let _ = socket.close(None).await;
        }
        ConnectionEnd::Rejected(reason) => {
            policy_close(&mut socket, reason).await;
        }
        ConnectionEnd::HeartbeatTimeout => {}
        ConnectionEnd::Disconnected | ConnectionEnd::Failed(_) | ConnectionEnd::DetachFailed(_) => {
        }
    }
    end
}

async fn close_heartbeat_timeout<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tracing::warn!(target: "nanocode_code_mode::attachment", stage = "attachment.socket.close_sent", close_code = u16::from(CloseCode::Restart), reason_code = "heartbeat_timeout", "attachment close sent");
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        socket.close(Some(CloseFrame {
            code: CloseCode::Restart,
            reason: HEARTBEAT_TIMEOUT_REASON.into(),
        })),
    )
    .await;
}

async fn next_handshake_frame<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    commands: &mut mpsc::Receiver<Command>,
) -> Result<EndpointFrame, ConnectionEnd>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::select! {
        command = commands.recv() => match command { Some(Command::Detach) | None => Err(ConnectionEnd::Detached) },
        frame = tokio::time::timeout(HANDSHAKE_TIMEOUT, next_frame(socket)) => match frame {
            Ok(frame) => frame,
            Err(_) => {
                tracing::warn!(target: "nanocode_code_mode::attachment", stage = "attachment.ready_timeout", reason_code = "ready_timeout", "attachment readiness timed out");
                Err(ConnectionEnd::Failed(AttachmentError::Transport("timed out waiting for attachment readiness".into())))
            },
        },
    }
}

async fn incoming_frame<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    incoming: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> Result<Option<EndpointFrame>, ConnectionEnd>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if let Some(Ok(Message::Close(frame))) = &incoming {
        tracing::info!(target: "nanocode_code_mode::attachment", stage = "attachment.socket.close_received", close_code = frame.as_ref().map(|frame| u16::from(frame.code)), reason_code = "remote_close", "attachment close received");
    }
    match incoming {
        Some(Ok(Message::Text(text))) => EndpointFrame::parse(&text)
            .map(Some)
            .map_err(|reason| ConnectionEnd::Rejected(reason.into())),
        Some(Ok(Message::Ping(payload))) => {
            socket.send(Message::Pong(payload)).await.map_err(|error| {
                ConnectionEnd::Failed(AttachmentError::Transport(error.to_string().into()))
            })?;
            Ok(None)
        }

        Some(Ok(Message::Pong(_))) => Ok(None),
        Some(Ok(Message::Close(Some(frame)))) if frame.code == CloseCode::Policy => {
            Err(ConnectionEnd::Rejected(if frame.reason.is_empty() {
                "endpoint rejected the attachment".into()
            } else {
                frame.reason.to_string().into()
            }))
        }
        Some(Ok(Message::Close(_))) | None => Err(ConnectionEnd::Disconnected),
        Some(Ok(_)) => Err(ConnectionEnd::Rejected(
            "endpoint sent a non-text frame".into(),
        )),
        Some(Err(error)) => Err(ConnectionEnd::Failed(AttachmentError::Transport(
            error.to_string().into(),
        ))),
    }
}

async fn next_frame<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<EndpointFrame, ConnectionEnd>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let incoming = socket.next().await;
        match incoming_frame(socket, incoming).await? {
            Some(frame) => return Ok(frame),
            None => continue,
        }
    }
}

async fn reject<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    reason: impl Into<Box<str>>,
) -> ConnectionEnd
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let reason = reason.into();
    policy_close(socket, &reason).await;
    ConnectionEnd::Rejected(reason)
}

async fn policy_close<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>, reason: &str)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tracing::warn!(target: "nanocode_code_mode::attachment", stage = "attachment.socket.close_sent", close_code = u16::from(CloseCode::Policy), reason_code = "protocol_rejected", "attachment close sent");
    let reason = bounded(reason);
    let reason = reason
        .get(..reason.len().min(123))
        .unwrap_or("attachment protocol violation");
    let _ = socket
        .close(Some(CloseFrame {
            code: CloseCode::Policy,
            reason: reason.into(),
        }))
        .await;
}

async fn send_diagnostics<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    call_id: &str,
    clock: &Arc<Atom<CallClock>>,
    prepared: bool,
) -> Result<(), AttachmentError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let now = Instant::now();
    let phases = clock.update(|clock| {
        let phases = [
            (DiagnosticStage::Received, Some(clock.received)),
            (DiagnosticStage::ExecutionStarted, clock.execution_started),
            (DiagnosticStage::ExecutionFinished, clock.execution_finished),
            (DiagnosticStage::ResultPrepared, prepared.then_some(now)),
        ];
        let mut pending = Vec::with_capacity(4);
        for (index, (stage, at)) in phases.into_iter().enumerate() {
            if let Some(at) = at
                && clock.diagnostics_sent & (1 << index) == 0
            {
                clock.diagnostics_sent |= 1 << index;
                let elapsed_ms = (at.saturating_duration_since(clock.received).as_secs_f64()
                    * 1000.0)
                    .min(9_007_199_254_740_991.0);
                pending.push((stage, elapsed_ms, clock.span.clone()));
            }
        }
        pending
    });
    let pending_count = phases.len();
    for (stage, elapsed_ms, span) in phases {
        if matches!(
            stage,
            DiagnosticStage::Received | DiagnosticStage::ResultPrepared
        ) {
            span.in_scope(|| tracing::info!(target: "nanocode_code_mode::attachment", stage = stage.name(), elapsed_ms, "attachment execution phase"));
        }
        feed(
            socket,
            &RunnerFrame::Diagnostic {
                call_id: call_id.into(),
                stage,
                elapsed_ms,
            },
        )
        .await?;
    }

    if !prepared && pending_count > 0 {
        socket
            .flush()
            .await
            .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
    }
    Ok(())
}

async fn send_retained<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    call: &RetainedCall,
    replayed: bool,
) -> Result<(), AttachmentError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let started = Instant::now();
    let received = call.timing.read(|clock| clock.received);
    tracing::info!(target: "nanocode_code_mode::attachment",
        stage = "attachment.result_send_started", transport_call_id = call.identity.call_id.as_ref(),
        replayed, elapsed_ms = started.duration_since(received).as_secs_f64() * 1000.0,
        "attachment terminal transmission started");
    send_diagnostics(socket, &call.identity.call_id, &call.timing, true).await?;
    socket
        .feed(Message::Text(
            call.receipt
                .as_ref()
                .expect("terminal receipt")
                .clone()
                .into(),
        ))
        .await
        .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
    let flush_started = Instant::now();
    tracing::info!(target: "nanocode_code_mode::attachment",
        stage = "attachment.result_flush_started", transport_call_id = call.identity.call_id.as_ref(),
        replayed, feed_ms = flush_started.duration_since(started).as_secs_f64() * 1000.0,
        elapsed_ms = flush_started.duration_since(received).as_secs_f64() * 1000.0,
        "attachment terminal flush started");
    socket
        .flush()
        .await
        .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
    let flushed = Instant::now();

    tracing::info!(target: "nanocode_code_mode::attachment",
        stage = if replayed { "attachment.result_replayed" } else { "attachment.result_sent" },
        transport_call_id = call.identity.call_id.as_ref(),
        feed_ms = flush_started.duration_since(started).as_secs_f64() * 1000.0,
        flush_ms = flushed.duration_since(flush_started).as_secs_f64() * 1000.0,
        send_ms = flushed.duration_since(started).as_secs_f64() * 1000.0,
        elapsed_ms = flushed.duration_since(received).as_secs_f64() * 1000.0,
        "attachment terminal receipt flushed");
    Ok(())
}

fn encode_result(
    call_id: &str,
    outcome: &Value,
    clock: &Arc<Atom<CallClock>>,
) -> Result<String, AttachmentError> {
    let encode_started = Instant::now();
    let mut timing = clock.read(|clock| clock.snapshot(encode_started));

    let mut text = serde_json::to_string(&RunnerFrame::Result {
        call_id: call_id.into(),
        outcome: Cow::Borrowed(outcome),
        timing: None,
    })
    .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
    let encoded_at = Instant::now();
    let encode_ms = encoded_at.duration_since(encode_started).as_secs_f64() * 1000.0;
    timing.result_encode_ms += encode_ms;
    timing.host_elapsed_ms += encode_ms;
    text.pop();
    text.push_str(",\"timing\":");
    text.push_str(
        &serde_json::to_string(&timing)
            .map_err(|error| AttachmentError::Transport(error.to_string().into()))?,
    );
    text.push('}');
    Ok(text)
}

async fn send<S, T: serde::Serialize>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    frame: &T,
) -> Result<(), AttachmentError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let text = serde_json::to_string(frame)
        .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| AttachmentError::Transport(error.to_string().into()))
}

async fn feed<S, T: serde::Serialize>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    frame: &T,
) -> Result<(), AttachmentError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let text = serde_json::to_string(frame)
        .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
    socket
        .feed(Message::Text(text.into()))
        .await
        .map_err(|error| AttachmentError::Transport(error.to_string().into()))
}

fn unavailable(message: &str) -> Value {
    json!({"status":"unavailable", "message":bounded(message)})
}
fn ambiguous(message: &str) -> Value {
    json!({"status":"ambiguous", "message":bounded(message)})
}
fn bounded_completed_failure(
    message: &str,
    output_byte_budget: u64,
) -> (Value, AttachmentCallOutcome) {
    let output = json!({"output":bounded(message),"success":false,"structured_result":null,"metadata":null,"process_trace":null});
    if serde_json::to_vec(&output).is_ok_and(|bytes| bytes.len() as u64 <= output_byte_budget) {
        (
            json!({"status":"completed", "output":output}),
            AttachmentCallOutcome::Completed,
        )
    } else {
        (
            ambiguous("tool output exceeded byte budget"),
            AttachmentCallOutcome::Ambiguous,
        )
    }
}
fn bounded(message: &str) -> &str {
    message
        .get(..message.len().min(2048))
        .unwrap_or("tool failed")
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(u64::MAX)
        })
}
fn emit(events: &mpsc::Sender<AttachmentEvent>, event: AttachmentEvent) {
    trace_attachment_event(&event);
    let _ = events.try_send(event);
}

fn trace_attachment_event(event: &AttachmentEvent) {
    match event {
        AttachmentEvent::Connecting => tracing::info!(
            target: "nanocode_code_mode::attachment",
            stage = "attachment.connecting",
            "connecting attachment"
        ),
        AttachmentEvent::Retrying { reason, delay_ms } => tracing::info!(
            target: "nanocode_code_mode::attachment", reason = reason.as_ref(), delay_ms,
            "retrying attachment"
        ),
        AttachmentEvent::Attached => tracing::info!(
            target: "nanocode_code_mode::attachment",
            stage = "attachment.attached",
            "attachment accepted"
        ),
        AttachmentEvent::CatalogPublished { tool_count } => tracing::info!(
            target: "nanocode_code_mode::attachment",
            stage = "attachment.catalog_published",
            tool.count = tool_count,
            "attachment catalog published"
        ),
        AttachmentEvent::Detached { .. } => tracing::info!(
            target: "nanocode_code_mode::attachment",
            stage = "attachment.detached",
            "attachment detached"
        ),
        AttachmentEvent::Fenced { .. } => tracing::warn!(
            target: "nanocode_code_mode::attachment",
            stage = "attachment.fenced",
            "attachment fenced"
        ),
        AttachmentEvent::CallStarted { .. } | AttachmentEvent::CallCompleted { .. } => {}
    }
}

async fn accept_runner(
    listener: &tokio::net::TcpListener,
    authorization: &str,
    events: &mpsc::Sender<AttachmentEvent>,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        http::Response<Option<Vec<u8>>>,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    loop {
        let (stream, _) = listener.accept().await?;
        stream.set_nodelay(true)?;
        let authenticate =
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
             mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                let header = request.headers().get(http::header::AUTHORIZATION).and_then(|value| value.to_str().ok());
                let protocol = request.headers().get(http::header::SEC_WEBSOCKET_PROTOCOL).and_then(|value| value.to_str().ok()).and_then(|value| {
                    let mut values = value.split(',').map(str::trim);
                    if values.next() == Some("bearer") { values.next() } else { None }
                });
                if header == Some(authorization) || protocol.is_some_and(|token| authorization.strip_prefix("Bearer ") == Some(token)) {
                    if protocol.is_some() {
                        response.headers_mut().insert(http::header::SEC_WEBSOCKET_PROTOCOL, http::HeaderValue::from_static("bearer"));
                    }
                    Ok(response)
                } else {
                    emit(
                        events,
                        AttachmentEvent::Retrying {
                            reason: "authentication_failed".into(),
                            delay_ms: 0,
                        },
                    );
                    let mut rejection =
                        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(
                            Some("invalid runner credential".into()),
                        );
                    *rejection.status_mut() = http::StatusCode::UNAUTHORIZED;
                    Err(rejection)
                }
            };
        if let Ok(Ok(socket)) = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            tokio_tungstenite::accept_hdr_async(
                tokio_tungstenite::MaybeTlsStream::Plain(stream),
                authenticate,
            ),
        )
        .await
        {
            return Ok((socket, http::Response::new(None)));
        }
    }
}
