#[cfg(not(target_family = "wasm"))]
#[path = "support/stream.rs"]
mod stream;

#[cfg(not(target_family = "wasm"))]
mod native {
    use std::{error::Error, path::PathBuf, time::Duration};

    use nanocode::{
        Claude, ClaudeClient, ClaudeHarness, ClaudeModel, Codex, CodexHarness, HarnessFamily,
        JsonlStore, Model, ModelSelection, Nanocode, PromptRequest, Thinking, Tools,
        agent::ExecutionEnvironment,
        claude::ClaudeEnvironment,
        durability::{SessionStore, TurnStatus},
    };

    type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

    const HELP: &str = "Durable inference and subagents with optional remote Code Mode.

Usage: cargo run --locked -p nanocode --example brain -- [OPTIONS] [PROMPT...]

Options:
  --resume ID        Reopen a session and replay interrupted prompts before PROMPT
  --store PATH       JSONL store (default: ~/.nanocode/sessions.jsonl, or under $NANOCODE_HOME)
  --harness NAME     codex or claude (otherwise inferred from --model)
  --model NAME       SDK model name or alias (default: luna; claude: haiku)
  --thinking LEVEL   low, medium, high, xhigh, or max (default: low)
  --connect URL      Connect to a remote Code Mode executor over ws:// or wss://
  --listen ADDRESS   Accept a remote Code Mode executor at HOST:PORT
  --token-env NAME   Remote token environment variable (default: NANOCODE_RUNNER_TOKEN)
  --token-file PATH  Read the remote token from a file instead
  --max-frame-bytes N Remote frame size policy (required with --connect/--listen)
  --timeout-ms N Remote request timeout in milliseconds (required with --connect/--listen)
  --                 Treat remaining arguments as prompt text
  -h, --help         Show help without accessing credentials or storage

Both providers are enabled using their normal SDK credential discovery.
No shell, filesystem, CUA, or runner management tools run on this brain.
Remote execution requires a separately running executor. Reconnect it on resume.
Without a prompt, a new session drafts a book swap plan using one subagent.
With --resume and no prompt, only interrupted prompts are replayed.
Session IDs appear on stderr before inference. Answers stream to stdout;
reasoning summaries and tool calls stream to stderr.

Examples:
  cargo run --locked -p nanocode --example brain --
  cargo run --locked -p nanocode --example brain -- --resume SESSION_ID 'Shorten the plan'
  cargo run --locked -p nanocode --example brain -- --model haiku 'Explain durable agents'
  cargo run --locked -p nanocode --example brain -- --connect ws://127.0.0.1:9000 --max-frame-bytes 67108864 --timeout-ms 120000 'Inspect the remote workspace'
";

    const DEMO: &str = "Draft a concise plan for organizing a small community book swap. Use Code Mode to ask one subagent for a practical checklist, inheriting this model and low reasoning effort. Give it a typed output_contract with a checklist array of strings. Wait for its accepted result, then combine its suggestions into a short final plan. No browsing or machine access is needed.";

    #[derive(Default)]
    struct Args {
        resume: Option<String>,
        store: Option<PathBuf>,
        harness: Option<String>,
        model: Option<String>,
        thinking: Option<String>,
        connect: Option<String>,
        listen: Option<String>,
        token_env: Option<String>,
        token_file: Option<String>,
        max_frame_bytes: Option<String>,
        timeout_ms: Option<String>,
        prompt: Vec<String>,
    }

    fn parse() -> Result<Option<Args>> {
        let values: Vec<String> = std::env::args().skip(1).collect();
        if values
            .iter()
            .take_while(|arg| arg.as_str() != "--")
            .any(|arg| arg == "--help" || arg == "-h")
        {
            print!("{HELP}");
            return Ok(None);
        }
        let mut args = Args::default();
        let mut values = values.into_iter();
        while let Some(arg) = values.next() {
            if arg == "--" {
                args.prompt.extend(values);
                break;
            }
            let slot = match arg.as_str() {
                "--resume" => &mut args.resume,
                "--harness" => &mut args.harness,
                "--model" => &mut args.model,
                "--thinking" => &mut args.thinking,
                "--connect" => &mut args.connect,
                "--listen" => &mut args.listen,
                "--token-env" => &mut args.token_env,
                "--token-file" => &mut args.token_file,
                "--max-frame-bytes" => &mut args.max_frame_bytes,
                "--timeout-ms" => &mut args.timeout_ms,
                "--store" => {
                    let path = values.next().ok_or("--store requires a path")?;
                    if path.trim().is_empty() {
                        return Err("--store requires a nonempty path".into());
                    }
                    if args.store.replace(path.into()).is_some() {
                        return Err("--store was specified more than once".into());
                    }
                    continue;
                }
                _ if arg.starts_with('-') => {
                    return Err(format!("unknown option {arg}; see --help").into());
                }
                _ => {
                    args.prompt.push(arg);
                    continue;
                }
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
        if args.connect.is_some() && args.listen.is_some() {
            return Err("choose either --connect or --listen".into());
        }
        if args.token_env.is_some() && args.token_file.is_some() {
            return Err("choose either --token-env or --token-file".into());
        }
        if (args.token_env.is_some() || args.token_file.is_some())
            && args.connect.is_none()
            && args.listen.is_none()
        {
            return Err("--token-env and --token-file require --connect or --listen".into());
        }
        if args.resume.is_some()
            && (args.harness.is_some() || args.model.is_some() || args.thinking.is_some())
        {
            return Err("--resume restores the saved model and thinking; omit --harness, --model, and --thinking".into());
        }
        Ok(Some(args))
    }

    fn selection(args: &Args) -> Result<ModelSelection> {
        let model = args
            .model
            .as_deref()
            .map(str::parse::<ModelSelection>)
            .transpose()?;
        let family = match args.harness.as_deref() {
            None => model
                .as_ref()
                .map(ModelSelection::family)
                .unwrap_or(HarnessFamily::Codex),
            Some("codex") => HarnessFamily::Codex,
            Some("claude") => HarnessFamily::Claude,
            Some(name) => {
                return Err(format!("unsupported harness {name:?}; choose codex or claude").into());
            }
        };
        if let Some(model) = model {
            if model.family() != family {
                return Err("--harness conflicts with --model; omit --harness to infer it".into());
            }
            Ok(model)
        } else if family == HarnessFamily::Claude {
            Ok(ClaudeModel::Haiku55.into())
        } else {
            Ok(Model::Luna.into())
        }
    }

    async fn store(args: &Args) -> Result<SessionStore> {
        let store = match &args.store {
            Some(path) => {
                if let Some(parent) = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                {
                    tokio::fs::create_dir_all(parent).await?;
                }
                JsonlStore::open_async(path).await?
            }
            None => {
                let home = nanocode_home::AgentHome::from_env()?;
                JsonlStore::open_home_async(home.nanocode_home()).await?
            }
        };
        Ok(nanocode::session_store(store)?)
    }

    fn token(args: &Args) -> Result<String> {
        let token = if let Some(path) = &args.token_file {
            std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read --token-file: {error}"))?
        } else {
            let name = args.token_env.as_deref().unwrap_or("NANOCODE_RUNNER_TOKEN");
            std::env::var(name).map_err(|_| {
                format!("set {name} to the executor's shared token, or use --token-file")
            })?
        };
        let token = token.trim();
        if token.is_empty() {
            return Err("remote token is empty; provide the executor's shared token".into());
        }
        Ok(token.to_owned())
    }

    async fn remote(args: &Args) -> Result<Option<(nanocode::RemoteCodeModeHost, Option<String>)>> {
        use nanocode::code_mode::{
            attachment::AttachmentTarget,
            remote::{SocketPolicy, SocketTransport},
        };

        if args.connect.is_none() && args.listen.is_none() {
            if args.max_frame_bytes.is_some() || args.timeout_ms.is_some() {
                return Err("remote policy options require --connect or --listen".into());
            }
            return Ok(None);
        }
        let policy = SocketPolicy {
            max_frame_bytes: args
                .max_frame_bytes
                .as_deref()
                .ok_or("remote execution requires --max-frame-bytes N")?
                .parse()?,
            request_timeout: Duration::from_millis(
                args.timeout_ms
                    .as_deref()
                    .ok_or("remote execution requires --timeout-ms N")?
                    .parse()?,
            ),
        };
        if policy.max_frame_bytes == 0 || policy.request_timeout.is_zero() {
            return Err("remote frame size and request timeout must be positive".into());
        }
        let token = token(args)?;
        let transport = if let Some(endpoint) = &args.connect {
            SocketTransport::connect(AttachmentTarget::new(endpoint, token)?, policy).await?
        } else {
            let listener = tokio::net::TcpListener::bind(
                args.listen.as_deref().ok_or("missing --listen address")?,
            )
            .await?;
            eprintln!(
                "waiting for a Code Mode executor at {}",
                listener.local_addr()?
            );
            SocketTransport::accept(&listener, &token, policy).await?
        };
        eprintln!("remote Code Mode executor connected");
        let workspace = transport.workspace().map(str::to_owned);
        Ok(Some((transport.into_host(), workspace)))
    }

    async fn run(args: Args) -> Result<()> {
        let model = selection(&args)?;
        let thinking = args
            .thinking
            .as_deref()
            .unwrap_or("low")
            .parse::<Thinking>()?;
        let remote = remote(&args).await?;
        let store = store(&args).await?;
        let tools = Tools::builder().without_defaults().build()?;
        if tools.workspace_enabled()
            || tools.web_search_enabled()
            || tools.image_generation_enabled()
        {
            return Err("brain unexpectedly enabled default tools".into());
        }
        let mut codex = CodexHarness::from(nanocode::OpenAi::default());
        let mut claude = ClaudeHarness::from(ClaudeClient::default());
        if let Some(directory) = remote.as_ref().and_then(|(_, workspace)| workspace.clone()) {
            let working_directory = directory.clone();
            codex = codex.configure(move |builder| {
                Ok(builder.execution_environment(
                    ExecutionEnvironment::local().working_directory(working_directory.as_str()),
                ))
            });
            claude = claude.configure(move |builder| {
                Ok(builder
                    .environment(ClaudeEnvironment::new().working_directory(directory.as_str())))
            });
        }
        let mut builder = Nanocode::builder()
            .openai(codex)
            .claude(claude)
            .tools(tools)
            .without_cua()
            .runner_management(false)
            .durability(store.clone())
            .thinking(thinking);
        if let Some((host, _)) = remote {
            builder = builder.code_mode_host(host)?;
        }
        let runtime = if let Some(model) = model.as_codex() {
            builder
                .for_backend::<Codex>()
                .model(model)
                .build_async()
                .await?
        } else if let Some(model) = model.as_claude() {
            builder
                .for_backend::<Claude>()
                .model(model)
                .build_async()
                .await?
        } else {
            return Err("select a built-in Codex or Claude model".into());
        };
        let outcome = inference(&runtime, &store, &args).await;
        let shutdown = runtime.shutdown().await;
        outcome?;
        shutdown?;
        Ok(())
    }

    async fn inference(runtime: &Nanocode, store: &SessionStore, args: &Args) -> Result<()> {
        let pending = if let Some(id) = &args.resume {
            store
                .load(id)
                .await?
                .turns
                .into_iter()
                .filter(|turn| turn.status == TurnStatus::Pending)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let (agent, mut events) = match &args.resume {
            Some(id) => runtime.open_session(id).await?,
            None => runtime.spawn().build().await?,
        };
        eprintln!("session {}", agent.session_id());
        let outcome: Result<()> = async {
            for turn in pending {
                eprintln!("resuming interrupted turn {}", turn.id);
                let prompt = agent.decode_durable_prompt(turn.input)?;
                crate::stream::prompt(
                    &agent,
                    &mut events,
                    PromptRequest::new(prompt).request_id(turn.id),
                )
                .await?;
            }
            let prompt = args.prompt.join(" ");
            let prompt = if prompt.trim().is_empty() && args.resume.is_none() {
                DEMO
            } else {
                &prompt
            };
            if !prompt.trim().is_empty() {
                crate::stream::prompt(&agent, &mut events, prompt).await?;
            }
            agent.flush().await?;
            Ok(())
        }
        .await;
        outcome
    }

    pub fn main() -> Result<()> {
        let Some(args) = parse()? else {
            return Ok(());
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(run(args))
    }
}

#[cfg(not(target_family = "wasm"))]
fn main() {
    if let Err(error) = native::main() {
        eprintln!("brain: {error}\nUse --help for invocation and provider setup details.");
        std::process::exit(1);
    }
}

#[cfg(target_family = "wasm")]
fn main() {}
