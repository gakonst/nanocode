#[cfg(not(target_family = "wasm"))]
mod native {
    use std::{
        error::Error,
        io::{BufRead, ErrorKind},
        net::IpAddr,
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    };

    use nanocode::code_mode::{
        Tools,
        attachment::AttachmentTarget,
        remote::{SocketPolicy, serve_accept, serve_connect},
    };
    use tokio::{
        net::{TcpListener, TcpStream},
        sync::watch,
        task::{JoinError, JoinSet},
    };

    type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

    const HELP: &str = "Machine-backed Code Mode executor for a separate durable brain.

Usage: cargo run --locked -p nanocode --example runner -- [OPTIONS]

Options:
  --listen ADDRESS     Accept the brain connection on a loopback HOST:PORT
  --connect URL        Connect to a listening brain over ws:// or wss://
  --multi-session      With --listen, keep serving independent concurrent brain
                       connections until stopped instead of exiting after one
  --workspace PATH     Machine workspace (default: current directory)
  --token-env NAME     Credential environment variable (default: NANOCODE_RUNNER_TOKEN)
  --token-stdin        Read the credential from the first stdin line before starting
  --max-frame-bytes N   Explicit maximum inbound/outbound frame size
  --timeout-ms N        Explicit handshake, request, callback and session cleanup deadline
  -h, --help           Show help

Choose exactly one of --listen and --connect. Both policy values are required.
Use a TLS proxy for connections across machines. Share the token out of band.
The machine runs the evaluator and workspace tools. Inference, subagents and
durability remain on the brain. Every multi-session connection authenticates
independently and shares the workspace. On Linux, --multi-session marks the
process non-dumpable before reading the credential. Stop this runner with Ctrl-C or SIGTERM;
a second signal during multi-session shutdown aborts the remaining sessions.
";

    struct Args {
        listen: Option<String>,
        connect: Option<String>,
        multi_session: bool,
        workspace: PathBuf,
        token: TokenSource,
        policy: SocketPolicy,
    }

    enum TokenSource {
        Env(String),
        Stdin,
    }

    fn parse() -> Result<Option<Args>> {
        let values: Vec<String> = std::env::args().skip(1).collect();
        if values.iter().any(|arg| arg == "--help" || arg == "-h") {
            print!("{HELP}");
            return Ok(None);
        }
        let mut listen = None;
        let mut connect = None;
        let mut multi_session = false;
        let mut token_stdin = false;
        let mut workspace = None;
        let mut token_env = None;
        let mut max_frame_bytes = None;
        let mut timeout_ms = None;
        let mut values = values.into_iter();
        while let Some(arg) = values.next() {
            let slot = match arg.as_str() {
                "--multi-session" | "--token-stdin" => {
                    let flag = if arg == "--multi-session" {
                        &mut multi_session
                    } else {
                        &mut token_stdin
                    };
                    if std::mem::replace(flag, true) {
                        return Err(format!("{arg} was specified more than once").into());
                    }
                    continue;
                }
                "--listen" => &mut listen,
                "--connect" => &mut connect,
                "--workspace" => &mut workspace,
                "--token-env" => &mut token_env,
                "--max-frame-bytes" => &mut max_frame_bytes,
                "--timeout-ms" => &mut timeout_ms,
                _ => return Err(format!("unknown option {arg}; see --help").into()),
            };
            let value = values
                .next()
                .ok_or_else(|| format!("{arg} requires a value"))?;
            if value.trim().is_empty() {
                return Err(format!("{arg} requires a nonempty value").into());
            }
            if slot.replace(value).is_some() {
                return Err(format!("{arg} was specified more than once").into());
            }
        }
        if listen.is_some() == connect.is_some() {
            return Err("choose exactly one of --listen and --connect".into());
        }
        if multi_session && listen.is_none() {
            return Err("--multi-session requires --listen".into());
        }
        if token_stdin && token_env.is_some() {
            return Err("choose at most one of --token-env and --token-stdin".into());
        }
        let max_frame_bytes = max_frame_bytes
            .ok_or("--max-frame-bytes is required; choose a size for your tool inputs and outputs")?
            .parse::<usize>()?;
        let timeout_ms = timeout_ms
            .ok_or("--timeout-ms is required; choose a deadline for your workload")?
            .parse::<u64>()?;
        if max_frame_bytes == 0 || timeout_ms == 0 {
            return Err("frame size and timeout must be positive".into());
        }
        Ok(Some(Args {
            listen,
            connect,
            multi_session,
            workspace: match workspace {
                Some(path) => path.into(),
                None => std::env::current_dir()?,
            },
            token: if token_stdin {
                TokenSource::Stdin
            } else {
                TokenSource::Env(token_env.unwrap_or_else(|| "NANOCODE_RUNNER_TOKEN".into()))
            },
            policy: SocketPolicy {
                max_frame_bytes,
                request_timeout: Duration::from_millis(timeout_ms),
            },
        }))
    }

    #[cfg(unix)]
    struct Shutdown {
        interrupt: tokio::signal::unix::Signal,
        terminate: tokio::signal::unix::Signal,
    }

    #[cfg(unix)]
    impl Shutdown {
        fn install() -> Result<Self> {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
            })
        }

        async fn recv(&mut self) -> Result<&'static str> {
            nanocode_sync::select! {
                _ = self.interrupt.recv() => Ok("SIGINT"),
                _ = self.terminate.recv() => Ok("SIGTERM"),
            }
        }
    }

    #[cfg(not(unix))]
    struct Shutdown;

    #[cfg(not(unix))]
    impl Shutdown {
        fn install() -> Result<Self> {
            Ok(Self)
        }

        async fn recv(&mut self) -> Result<&'static str> {
            tokio::signal::ctrl_c().await?;
            Ok("Ctrl-C")
        }
    }

    fn read_token(source: &TokenSource) -> Result<String> {
        match source {
            TokenSource::Env(name) => {
                let token = std::env::var(name)
                    .map_err(|_| format!("set {name} to the shared runner credential"))?;
                if token.trim().is_empty() {
                    return Err(format!("{name} must contain a nonempty credential").into());
                }
                Ok(token)
            }
            TokenSource::Stdin => {
                let mut token = String::new();
                std::io::stdin().lock().read_line(&mut token)?;
                token.truncate(token.trim_end_matches(['\r', '\n']).len());
                if token.trim().is_empty() {
                    return Err("--token-stdin requires a nonempty credential line".into());
                }
                Ok(token)
            }
        }
    }

    async fn run(args: Args, token: String) -> Result<()> {
        let mut shutdown = Shutdown::install()?;
        let workspace = tokio::fs::canonicalize(&args.workspace).await?;
        let tools = Tools::builder()
            .web_search(false)
            .image_generation(false)
            .add(nanocode::tools::WorkspaceTools::new(workspace.clone()))
            .build()?;
        if let Some(address) = args.listen {
            let listener = TcpListener::bind(&address).await?;
            if !listener.local_addr()?.ip().is_loopback() {
                return Err("plaintext listeners require a loopback address; use a TLS proxy for remote access".into());
            }
            if args.multi_session {
                eprintln!(
                    "runner listening ws://{} workspace {} multi-session",
                    listener.local_addr()?,
                    workspace.display()
                );
                return serve_sessions(
                    listener,
                    token.into(),
                    workspace.into(),
                    tools,
                    args.policy,
                    shutdown,
                )
                .await;
            }
            eprintln!(
                "runner listening ws://{} workspace {}",
                listener.local_addr()?,
                workspace.display()
            );
            nanocode_sync::select! {
                result = serve_accept(&listener, &token, &workspace, tools, args.policy) => result?,
                result = shutdown.recv() => { result?; },
            }
        } else if let Some(url) = args.connect {
            let target = AttachmentTarget::new(url, token)?;
            eprintln!(
                "runner connecting {} workspace {}",
                target.endpoint(),
                workspace.display()
            );
            nanocode_sync::select! {
                result = serve_connect(target, &workspace, tools, args.policy) => result?,
                result = shutdown.recv() => { result?; },
            }
        }
        Ok(())
    }

    struct Session {
        id: u64,
        host: IpAddr,
        token: Arc<str>,
        workspace: Arc<Path>,
        tools: Tools,
        policy: SocketPolicy,
        stop: watch::Receiver<bool>,
    }

    fn retry_accept(issue: &std::io::Error) -> bool {
        #[cfg(target_os = "linux")]
        {
            if issue.raw_os_error().is_some_and(|errno| {
                [
                    libc::EPROTO,
                    libc::ENOPROTOOPT,
                    libc::EHOSTDOWN,
                    libc::ENONET,
                    libc::EOPNOTSUPP,
                ]
                .contains(&errno)
            }) {
                return true;
            }
        }
        matches!(
            issue.kind(),
            ErrorKind::ConnectionAborted
                | ErrorKind::ConnectionReset
                | ErrorKind::Interrupted
                | ErrorKind::NetworkDown
                | ErrorKind::NetworkUnreachable
                | ErrorKind::HostUnreachable
        )
    }

    fn report(joined: std::result::Result<(u64, Result<()>), JoinError>) {
        match joined {
            Ok((id, Ok(()))) => eprintln!("runner session {id} closed"),
            Ok((id, Err(issue))) => eprintln!("runner session {id} ended: {issue}"),
            Err(issue) => eprintln!("runner session task failed: {issue}"),
        }
    }

    async fn serve_sessions(
        listener: TcpListener,
        token: Arc<str>,
        workspace: Arc<Path>,
        tools: Tools,
        policy: SocketPolicy,
        mut shutdown: Shutdown,
    ) -> Result<()> {
        let host = listener.local_addr()?.ip();
        let (stop, stopped) = watch::channel(false);
        let mut sessions = JoinSet::new();
        let mut next = 0u64;
        let mut exhausted = false;
        let reason = loop {
            nanocode_sync::select! {
                reason = shutdown.recv() => break reason?,
                Some(joined) = sessions.join_next(), if !sessions.is_empty() => {
                    report(joined);
                    exhausted = false;
                },
                accepted = listener.accept(), if !exhausted => match accepted {
                    Ok((stream, peer)) => {
                        next += 1;
                        eprintln!("runner session {next} connected from {peer}");
                        let session = Session {
                            id: next,
                            host,
                            token: Arc::clone(&token),
                            workspace: Arc::clone(&workspace),
                            tools: tools.clone(),
                            policy,
                            stop: stopped.clone(),
                        };
                        sessions.spawn(async move { (session.id, session.serve(stream).await) });
                    }
                    Err(issue) if retry_accept(&issue) => eprintln!("runner accept failed: {issue}"),
                    Err(issue) if sessions.is_empty() => return Err(issue.into()),
                    Err(issue) => {
                        eprintln!("runner accept failed: {issue}; waiting for a session to close");
                        exhausted = true;
                    }
                },
            }
        };
        drop(listener);
        eprintln!(
            "runner stopping on {reason}; closing {} sessions",
            sessions.len()
        );
        stop.send_replace(true);
        loop {
            nanocode_sync::select! {
                joined = sessions.join_next() => match joined {
                    Some(joined) => report(joined),
                    None => return Ok(()),
                },
                reason = shutdown.recv() => {
                    eprintln!("runner aborting {} sessions on {}", sessions.len(), reason?);
                    sessions.shutdown().await;
                    return Ok(());
                },
            }
        }
    }

    impl Session {
        async fn serve(mut self, brain: TcpStream) -> Result<()> {
            brain.set_nodelay(true)?;
            let relay = TcpListener::bind((self.host, 0)).await?;
            let upstream = TcpStream::connect(relay.local_addr()?).await?;
            upstream.set_nodelay(true)?;
            let serve = serve_accept(
                &relay,
                &self.token,
                &self.workspace,
                self.tools,
                self.policy,
            );
            tokio::pin!(serve);
            let mut bridge = Box::pin(bridge(brain, upstream));
            let bridged = nanocode_sync::select! {
                result = &mut serve => {
                    let _ = tokio::time::timeout(self.policy.request_timeout, bridge).await;
                    return result.map_err(Into::into);
                },
                result = &mut bridge => result.err(),
                () = async { let _ = self.stop.wait_for(|stop| *stop).await; } => None,
            };
            drop(bridge);
            tokio::time::timeout(self.policy.request_timeout, serve)
                .await
                .map_err(|_| "session cleanup exceeded --timeout-ms")??;
            bridged.map_or(Ok(()), |issue| Err(issue.into()))
        }
    }

    async fn bridge(mut brain: TcpStream, mut upstream: TcpStream) -> std::io::Result<()> {
        tokio::io::copy_bidirectional(&mut brain, &mut upstream)
            .await
            .map(drop)
    }

    pub fn main() -> Result<()> {
        let Some(args) = parse()? else {
            return Ok(());
        };
        #[cfg(target_os = "linux")]
        if args.multi_session {
            nanocode_platform::unix::forbid_core_dumps()
                .map_err(|issue| format!("failed to mark the runner non-dumpable: {issue}"))?;
        }
        let token = read_token(&args.token)?;
        let runtime = if args.multi_session {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
        } else {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
        };
        runtime.block_on(run(args, token))
    }
}

#[cfg(not(target_family = "wasm"))]
fn main() {
    if let Err(error) = native::main() {
        eprintln!("runner: {error}\nUse --help for invocation details.");
        std::process::exit(1);
    }
}

#[cfg(target_family = "wasm")]
fn main() {}
