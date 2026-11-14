use std::{
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant},
};

use tokio::net::TcpListener;

use super::native::{
    Dial, DialStages, Role, Served, SocketPolicy, dial_staged, error, hand_core, request_headers,
    serve, serve_tracked_stream, serving, validate_policy,
};
use super::pricing::{HandPricing, Meter, Priced};
use super::{ExecutorOptions, HandActivity};
use crate::embedded::CodeModeHostError;
use nanocode_code_mode::remote::{HandCore, HandHostInfo, HostInfoLimits, HostInfoSource};
use nanocode_sync::mpsc;

#[derive(Clone, Debug)]
pub enum HandEndpoint {
    Connect(String),
    Listen(SocketAddr),
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum HandEvent {
    Connecting {
        endpoint: String,
    },
    Connected {
        endpoint: String,
        dialed: DialStages,
    },
    Moving {
        endpoint: String,
    },
    Listening {
        endpoint: String,
    },
    BrainConnected {
        peer: SocketAddr,
    },
    Disconnected {
        reason: String,
    },
    Retrying {
        delay: Duration,
    },
    Revoked,
    Replaced,
    Rejected {
        status: u16,
    },
    Unconfirmed {
        status: u16,
        retry: Duration,
    },
}

pub type ConfirmRefusal =
    std::sync::Arc<dyn Fn(u16) -> nanocode_sync::future::BoxFuture<'static, bool> + Send + Sync>;

pub struct Hand {
    endpoint: HandEndpoint,
    token: String,
    name: Option<String>,
    headers: Vec<(String, String)>,
    directory: Option<PathBuf>,
    tools: crate::Tools,
    policy: SocketPolicy,
    activity: HandActivity,
    callback_hold: Option<Duration>,
    device: Option<std::sync::Arc<dyn super::HandCapabilities>>,
    tools_only: bool,
    environment: Option<nanocode_code_mode::remote::HandEnvironment>,
    host_info: Option<HostInfoSource>,
    host_info_limits: HostInfoLimits,
    pricing: Option<HandPricing>,
    confirm: Option<(ConfirmRefusal, Duration)>,
    runtime: Option<String>,
    version: Option<String>,
}

impl Hand {
    pub fn connect(url: impl Into<String>, token: impl Into<String>, tools: crate::Tools) -> Self {
        Self::new(HandEndpoint::Connect(url.into()), token.into(), tools)
    }

    pub fn listen(address: SocketAddr, token: impl Into<String>, tools: crate::Tools) -> Self {
        Self::new(HandEndpoint::Listen(address), token.into(), tools)
    }

    fn new(endpoint: HandEndpoint, token: String, tools: crate::Tools) -> Self {
        Self {
            endpoint,
            token,
            name: None,
            headers: Vec::new(),
            directory: None,
            activity: serving(&tools),
            tools,
            policy: SocketPolicy::runner(),
            callback_hold: None,
            device: None,
            tools_only: false,
            environment: None,
            host_info: None,
            host_info_limits: HostInfoLimits::default(),
            pricing: None,
            confirm: None,
            runtime: None,
            version: None,
        }
    }

    #[must_use]
    pub fn resume_runtime(mut self, runtime: impl Into<String>) -> Self {
        self.runtime = Some(runtime.into());
        self
    }

    #[must_use]
    pub fn confirm_refusals(mut self, confirm: ConfirmRefusal, retry: Duration) -> Self {
        self.confirm = Some((confirm, retry));
        self
    }

    #[must_use]
    pub fn pricing(mut self, pricing: HandPricing) -> Self {
        self.pricing = Some(pricing);
        self
    }

    #[must_use]
    pub fn capabilities(mut self, device: std::sync::Arc<dyn super::HandCapabilities>) -> Self {
        self.device = Some(device);
        self
    }

    #[must_use]
    pub const fn tools_only(mut self) -> Self {
        self.tools_only = true;
        self
    }

    #[must_use]
    pub fn environment(mut self, environment: nanocode_code_mode::remote::HandEnvironment) -> Self {
        self.environment = Some(environment);
        self
    }

    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    #[must_use]
    pub fn host_info(self, host_info: HandHostInfo) -> Self {
        self.host_info_source(std::sync::Arc::new(move || {
            let host_info = host_info.clone();
            Box::pin(async move { host_info })
        }))
    }

    #[must_use]
    pub fn host_info_source(mut self, source: HostInfoSource) -> Self {
        self.host_info = Some(source);
        self
    }

    #[must_use]
    pub const fn host_info_limits(mut self, limits: HostInfoLimits) -> Self {
        self.host_info_limits = limits;
        self
    }

    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    #[must_use]
    pub const fn callback_hold(mut self, hold: Duration) -> Self {
        self.callback_hold = Some(hold);
        self
    }

    #[must_use]
    pub const fn policy(mut self, policy: SocketPolicy) -> Self {
        self.policy = policy;
        self
    }

    #[must_use]
    pub fn activity(&self) -> HandActivity {
        self.activity.clone()
    }

    pub fn machine_name() -> Result<String, CodeModeHostError> {
        let host = host_name()?;
        let mut name = String::new();
        for character in host.trim().chars().flat_map(char::to_lowercase) {
            let next = if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            };
            if !(next == '-' && name.ends_with('-')) {
                name.push(next);
            }
        }
        let name = name.trim_matches('-');
        let name = if name.starts_with(|character: char| character.is_ascii_alphabetic()) {
            name.to_owned()
        } else {
            format!("hand-{name}")
        };
        let name = name
            .get(..MAX_NAME_BYTES)
            .unwrap_or(&name)
            .trim_end_matches('-')
            .to_owned();
        crate::attachment::RemoteRunner::new(&name).map_err(error)?;
        Ok(name)
    }

    pub fn run<F>(
        self,
        mut events: F,
    ) -> nanocode_sync::future::BoxFuture<'static, Result<(), CodeModeHostError>>
    where
        F: FnMut(HandEvent) + Send + 'static,
    {
        Box::pin(async move {
            validate_policy(self.policy)?;
            let _ = nanocode_process::raise_open_file_limit();
            let directory = match self.directory {
                Some(directory) => directory,
                None => std::env::current_dir().map_err(error)?,
            };
            let directory = tokio::fs::canonicalize(&directory).await.map_err(error)?;
            let name = match self.name {
                Some(name) => name,
                None => Self::machine_name()?,
            };
            crate::attachment::RemoteRunner::new(&name).map_err(error)?;
            let meter = self
                .pricing
                .clone()
                .map(|pricing| Meter::new(pricing, &directory));
            let device = match &self.pricing {
                Some(pricing) => Some(std::sync::Arc::new(Priced {
                    capability: pricing.capability(),
                    inner: self.device.clone(),
                })
                    as std::sync::Arc<dyn super::HandCapabilities>),
                None => self.device.clone(),
            };
            let mut resumed = self.runtime.is_some();
            let mut options = ExecutorOptions {
                name: Some(name),
                runtime_id: Some(
                    self.runtime
                        .unwrap_or_else(|| nanocode_platform::uuid_v4().to_string()),
                ),
                callback_timeout: None,
                callback_hold: self.callback_hold,
                device,
                tools_only: self.tools_only,
                environment: self.environment.clone(),
                moves: false,
                version: self.version.clone(),
                host_info: self.host_info.clone().map(|source| {
                    let limits = self.host_info_limits;
                    std::sync::Arc::new(move || {
                        let collected = source();
                        Box::pin(async move { collected.await.limited(limits) })
                            as crate::embedded::HostFuture<'static, HandHostInfo>
                    }) as HostInfoSource
                }),
            };
            let policy = self.policy;
            match self.endpoint {
                HandEndpoint::Connect(endpoint) => {
                    let target = crate::attachment::AttachmentTarget::new(endpoint, self.token)
                        .map_err(error)?;
                    let headers = request_headers(&self.headers)?;
                    let mut backoff = crate::attachment::INITIAL_TRANSPORT_BACKOFF;
                    options.moves = true;
                    let renew = |options: &ExecutorOptions| {
                        hand_core(
                            directory.clone(),
                            self.tools.clone(),
                            options.clone(),
                            self.activity.clone(),
                        )
                    };
                    let mut core = renew(&options);
                    let mut leaving: Option<Serving> = None;
                    loop {
                        let endpoint = target.endpoint().to_string();
                        events(if leaving.is_some() {
                            HandEvent::Moving {
                                endpoint: endpoint.clone(),
                            }
                        } else {
                            HandEvent::Connecting {
                                endpoint: endpoint.clone(),
                            }
                        });
                        let dialing = dial_staged(&target, &headers, policy);
                        let mut dialing = std::pin::pin!(dialing);
                        let dialed = loop {
                            nanocode_sync::select! {
                                dialed = &mut dialing => break dialed,
                                served = left(&mut leaving) => {
                                    leaving = None;
                                    if let Next::Stop = settle(served, Settled::Left, &mut core, &mut options, &renew, &mut events).await {
                                        return Ok(());
                                    }
                                }
                            }
                        };
                        let mut fresh = false;
                        match dialed {
                            Ok((socket, dialed)) => {
                                let started = Instant::now();
                                let role = if leaving.is_some() {
                                    Role::Standby
                                } else {
                                    Role::Current
                                };
                                let mut link =
                                    Serving::spawn(socket, &core, policy, meter.clone(), role);
                                let ended = loop {
                                    nanocode_sync::select! {
                                        served = ended(&mut link.task) => break Phase::Ended(served),
                                        Some(note) = link.notes.recv() => match note {
                                            Note::Heard => {
                                                events(HandEvent::Connected {
                                                    endpoint: endpoint.clone(),
                                                    dialed,
                                                });
                                            }
                                            Note::Moved => break Phase::Moved,
                                        },
                                        served = left(&mut leaving) => {
                                            leaving = None;
                                            match settle(served, Settled::Left, &mut core, &mut options, &renew, &mut events).await {
                                                Next::Stop => return Ok(()),
                                                Next::Redial { fenced: true } => break Phase::Fenced,
                                                Next::Redial { fenced: false } => {}
                                            }
                                        }
                                    }
                                };
                                let served = match ended {
                                    Phase::Ended(served) => served,
                                    Phase::Moved => {
                                        leaving = Some(link);
                                        continue;
                                    }
                                    Phase::Fenced => {
                                        drop(link);
                                        backoff = crate::attachment::MAX_TRANSPORT_BACKOFF;
                                        let delay =
                                            crate::attachment::reconnect_delay(&mut backoff);
                                        events(HandEvent::Retrying { delay });
                                        nanocode_platform::sleep(delay).await;
                                        continue;
                                    }
                                };
                                drop(link);
                                let retired = std::mem::take(&mut resumed)
                                    && served.result.as_ref().is_err_and(|issue| {
                                        issue.message() == super::HAND_REPLACED
                                    });
                                if retired {
                                    core.reset().await;
                                    options.runtime_id =
                                        Some(nanocode_platform::uuid_v4().to_string());
                                    core = renew(&options);
                                    events(HandEvent::Disconnected {
                                        reason: "the brain retired the resumed runtime; reconnecting as a new runtime".into(),
                                    });
                                    continue;
                                }
                                fresh = (served.heard && served.close_code == Some(RESTART_CLOSE))
                                    || started.elapsed() >= FRESH_CONNECTION;
                                if fresh {
                                    backoff = crate::attachment::INITIAL_TRANSPORT_BACKOFF;
                                }
                                match settle(
                                    served,
                                    Settled::Current,
                                    &mut core,
                                    &mut options,
                                    &renew,
                                    &mut events,
                                )
                                .await
                                {
                                    Next::Stop => return Ok(()),
                                    Next::Redial { fenced: true } => {
                                        leaving = None;
                                        fresh = false;
                                        backoff = crate::attachment::MAX_TRANSPORT_BACKOFF;
                                    }
                                    Next::Redial { fenced: false } => {}
                                }
                            }
                            Err(Dial::Rejected(issue, status)) => {
                                drop(leaving.take());
                                core.reset().await;
                                if let (Some(status @ (401 | 403)), Some((confirm, retry))) =
                                    (status, &self.confirm)
                                    && !confirm(status).await
                                {
                                    options.runtime_id =
                                        Some(nanocode_platform::uuid_v4().to_string());
                                    core = renew(&options);
                                    events(HandEvent::Unconfirmed {
                                        status,
                                        retry: *retry,
                                    });
                                    nanocode_platform::sleep(*retry).await;
                                    continue;
                                }
                                if let Some(status) = status {
                                    events(HandEvent::Rejected { status });
                                }
                                return Err(issue);
                            }
                            Err(Dial::Failed(issue)) => events(HandEvent::Disconnected {
                                reason: issue.to_string(),
                            }),
                        }
                        let delay = if fresh {
                            Duration::ZERO
                        } else {
                            crate::attachment::reconnect_delay(&mut backoff)
                        };
                        events(HandEvent::Retrying { delay });
                        if !delay.is_zero() {
                            nanocode_platform::sleep(delay).await;
                        }
                    }
                }
                HandEndpoint::Listen(address) => {
                    if !address.ip().is_loopback() {
                        return Err(error(
                            "plaintext hand listeners require a loopback address; use a TLS proxy for remote access",
                        ));
                    }
                    let listener = TcpListener::bind(address).await.map_err(error)?;
                    events(HandEvent::Listening {
                        endpoint: format!("ws://{}/", listener.local_addr().map_err(error)?),
                    });
                    let mut connections: tokio::task::JoinSet<Result<(), CodeModeHostError>> =
                        tokio::task::JoinSet::new();
                    loop {
                        nanocode_sync::select! {
                            Some(finished) = connections.join_next(), if !connections.is_empty() => {
                                let reason = match finished {
                                    Ok(Ok(())) => "the brain closed the connection".into(),
                                    Ok(Err(issue)) => issue.to_string(),
                                    Err(issue) => issue.to_string(),
                                };
                                events(HandEvent::Disconnected { reason });
                            }
                            accepted = listener.accept() => match accepted {
                                Ok((stream, peer)) => {
                                    events(HandEvent::BrainConnected { peer });
                                    let token = self.token.clone();
                                    let directory = directory.clone();
                                    let tools = self.tools.clone();
                                    let options = options.clone();
                                    let activity = self.activity.clone();
                                    let meter = meter.clone();
                                    nanocode_platform::spawn_joined(&mut connections, async move {
                                        serve_tracked_stream(stream, &token, &directory, tools, policy, options, activity, meter).await
                                    });
                                }
                                Err(issue) => events(HandEvent::Disconnected { reason: issue.to_string() }),
                            },
                        }
                    }
                }
            }
        })
    }
}

struct Serving {
    task: tokio::task::JoinHandle<Served>,
    notes: mpsc::UnboundedReceiver<Note>,
}

enum Note {
    Heard,
    Moved,
}

impl Serving {
    fn spawn(
        socket: super::native::DialedSocket,
        core: &HandCore,
        policy: SocketPolicy,
        meter: Option<std::sync::Arc<Meter>>,
        role: Role,
    ) -> Self {
        let (notify, notes) = mpsc::unbounded_channel();
        let core = core.clone();
        let task = nanocode_platform::spawn_task(async move {
            let moved = notify.clone();
            serve(
                socket,
                &core,
                policy,
                meter,
                role,
                move || drop(notify.send(Note::Heard)),
                move || drop(moved.send(Note::Moved)),
            )
            .await
        });
        Self { task, notes }
    }
}

async fn ended(task: &mut tokio::task::JoinHandle<Served>) -> Served {
    task.await.unwrap_or_else(|issue| Served {
        result: Err(error(issue)),
        close_code: None,
        heard: false,
    })
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn left(leaving: &mut Option<Serving>) -> Served {
    match leaving {
        Some(serving) => ended(&mut serving.task).await,
        None => std::future::pending().await,
    }
}

enum Phase {
    Ended(Served),
    Moved,
    Fenced,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Settled {
    Current,
    Left,
}

enum Next {
    Stop,
    Redial { fenced: bool },
}

async fn settle<F: FnMut(HandEvent)>(
    served: Served,
    settled: Settled,
    core: &mut HandCore,
    options: &mut ExecutorOptions,
    renew: &impl Fn(&ExecutorOptions) -> HandCore,
    events: &mut F,
) -> Next {
    match served.result {
        Err(issue)
            if issue.message() == nanocode_tool_host::attachment::ATTACHMENT_REVOKED_REASON =>
        {
            core.reset().await;
            events(HandEvent::Revoked);
            Next::Stop
        }
        Err(issue) if issue.message() == super::HAND_REPLACED => {
            core.reset().await;
            events(HandEvent::Replaced);
            Next::Stop
        }
        result if served.close_code == Some(POLICY_CLOSE) => {
            core.reset().await;
            options.runtime_id = Some(nanocode_platform::uuid_v4().to_string());
            *core = renew(options);
            let reason = result.err().map_or_else(
                || "no reason".to_owned(),
                |issue| issue.message().to_owned(),
            );
            events(HandEvent::Disconnected {
                reason: format!(
                    "the brain fenced this runtime with a policy close (1008): {reason}; reconnecting as a new runtime"
                ),
            });
            Next::Redial { fenced: true }
        }
        _ if settled == Settled::Left => Next::Redial { fenced: false },
        Ok(()) => {
            events(HandEvent::Disconnected {
                reason: "the brain closed the connection".into(),
            });
            Next::Redial { fenced: false }
        }
        Err(issue) => {
            events(HandEvent::Disconnected {
                reason: issue.to_string(),
            });
            Next::Redial { fenced: false }
        }
    }
}

const MAX_NAME_BYTES: usize = 48;
const POLICY_CLOSE: u16 = 1008;
const RESTART_CLOSE: u16 = 1012;
const FRESH_CONNECTION: Duration = Duration::from_secs(10);

#[cfg(unix)]
fn host_name() -> Result<String, CodeModeHostError> {
    nanocode_platform::unix::host_name()
        .map_err(error)?
        .into_string()
        .map_err(|_| error("the host name is not valid UTF-8"))
}

#[cfg(windows)]
fn host_name() -> Result<String, CodeModeHostError> {
    std::env::var("COMPUTERNAME").map_err(error)
}
