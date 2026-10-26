use std::{
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant},
};

use tokio::net::TcpListener;

use super::native::{
    Dial, Executor, ExecutorOptions, HandActivity, SocketPolicy, dial, error, request_headers,
    serve, serve_tracked_stream, validate_policy,
};
use crate::embedded::CodeModeHostError;

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
    /// The brain refused the hand for good: `status` is the HTTP status of a
    /// refused upgrade (401, 403, 410 or 426). `Hand::run` returns an error
    /// after it.
    Rejected {
        status: u16,
    },
}

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
            activity: HandActivity::serving(&tools),
            tools,
            policy: SocketPolicy::runner(),
            callback_hold: None,
        }
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
    ) -> futures_util::future::BoxFuture<'static, Result<(), CodeModeHostError>>
    where
        F: FnMut(HandEvent) + Send + 'static,
    {
        Box::pin(async move {
            validate_policy(self.policy)?;
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
            let mut options = ExecutorOptions {
                name: Some(name),
                runtime_id: Some(uuid::Uuid::new_v4().to_string()),
                callback_timeout: None,
                callback_hold: self.callback_hold,
            };
            let policy = self.policy;
            match self.endpoint {
                HandEndpoint::Connect(endpoint) => {
                    let target = crate::attachment::AttachmentTarget::new(endpoint, self.token)
                        .map_err(error)?;
                    let headers = request_headers(&self.headers)?;
                    let mut backoff = crate::attachment::INITIAL_TRANSPORT_BACKOFF;
                    let mut executor = Executor::new(
                        directory.clone(),
                        self.tools.clone(),
                        options.clone(),
                        self.activity.clone(),
                    );
                    loop {
                        let endpoint = target.endpoint().to_string();
                        events(HandEvent::Connecting {
                            endpoint: endpoint.clone(),
                        });
                        match dial(&target, &headers, policy).await {
                            Ok(socket) => {
                                let started = Instant::now();
                                // Connected only once the brain serves the socket: it may
                                // still refuse this runtime after the upgrade.
                                let served = serve(socket, &mut executor, policy, || {
                                    events(HandEvent::Connected { endpoint });
                                })
                                .await;
                                let mut fenced = false;
                                match served.result {
                                    Err(issue)
                                        if issue.message()
                                            == nanocode_tool_host::attachment::ATTACHMENT_REVOKED_REASON =>
                                    {
                                        executor.reset().await;
                                        events(HandEvent::Revoked);
                                        return Ok(());
                                    }
                                    Err(issue) if issue.message() == super::HAND_REPLACED => {
                                        executor.reset().await;
                                        events(HandEvent::Replaced);
                                        return Ok(());
                                    }
                                    result if served.close_code == Some(POLICY_CLOSE) => {
                                        executor.reset().await;
                                        options.runtime_id = Some(uuid::Uuid::new_v4().to_string());
                                        executor = Executor::new(
                                            directory.clone(),
                                            self.tools.clone(),
                                            options.clone(),
                                            self.activity.clone(),
                                        );
                                        fenced = true;
                                        let reason = result.err().map_or_else(
                                            || "no reason".to_owned(),
                                            |issue| issue.message().to_owned(),
                                        );
                                        events(HandEvent::Disconnected {
                                            reason: format!(
                                                "the brain fenced this runtime with a policy close (1008): {reason}; reconnecting as a new runtime"
                                            ),
                                        });
                                    }
                                    Ok(()) => events(HandEvent::Disconnected {
                                        reason: "the brain closed the connection".into(),
                                    }),
                                    Err(issue) => events(HandEvent::Disconnected {
                                        reason: issue.to_string(),
                                    }),
                                }
                                if started.elapsed() >= crate::attachment::STABLE_CONNECTION {
                                    backoff = crate::attachment::INITIAL_TRANSPORT_BACKOFF;
                                }
                                if fenced {
                                    backoff = crate::attachment::MAX_TRANSPORT_BACKOFF;
                                }
                            }
                            Err(Dial::Rejected(issue, status)) => {
                                executor.reset().await;
                                if let Some(status) = status {
                                    events(HandEvent::Rejected { status });
                                }
                                return Err(issue);
                            }
                            Err(Dial::Failed(issue)) => events(HandEvent::Disconnected {
                                reason: issue.to_string(),
                            }),
                        }
                        let delay = crate::attachment::reconnect_delay(&mut backoff);
                        events(HandEvent::Retrying { delay });
                        tokio::time::sleep(delay).await;
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
                    let mut connections = tokio::task::JoinSet::new();
                    loop {
                        tokio::select! {
                            accepted = listener.accept() => match accepted {
                                Ok((stream, peer)) => {
                                    events(HandEvent::BrainConnected { peer });
                                    let token = self.token.clone();
                                    let directory = directory.clone();
                                    let tools = self.tools.clone();
                                    let options = options.clone();
                                    let activity = self.activity.clone();
                                    connections.spawn(async move {
                                        serve_tracked_stream(stream, &token, &directory, tools, policy, options, activity).await
                                    });
                                }
                                Err(issue) => events(HandEvent::Disconnected { reason: issue.to_string() }),
                            },
                            Some(finished) = connections.join_next(), if !connections.is_empty() => {
                                let reason = match finished {
                                    Ok(Ok(())) => "the brain closed the connection".into(),
                                    Ok(Err(issue)) => issue.to_string(),
                                    Err(issue) => issue.to_string(),
                                };
                                events(HandEvent::Disconnected { reason });
                            }
                        }
                    }
                }
            }
        })
    }
}

const MAX_NAME_BYTES: usize = 48;
/// WebSocket close code 1008 (policy violation, RFC 6455 section 7.4.1).
const POLICY_CLOSE: u16 = 1008;

#[cfg(unix)]
fn host_name() -> Result<String, CodeModeHostError> {
    nix::unistd::gethostname()
        .map_err(error)?
        .into_string()
        .map_err(|_| error("the host name is not valid UTF-8"))
}

#[cfg(windows)]
fn host_name() -> Result<String, CodeModeHostError> {
    std::env::var("COMPUTERNAME").map_err(error)
}
