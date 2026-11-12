use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;

use nanocode_agent::events::{AgentEvent, AgentEventKind};
use nanocode_agent::{Agent, AgentEvents, Model};
use nanocode_backend_codex::client::{OpenAi, auth::OpenAiAuth, transport::ResponsesGateway};
use nanocode_backend_codex::{Codex, CodexHarness};
use nanocode_durability::{MemoryStore, SessionStore, SessionSummary};
use nanocode_store_jsonl::{JsonlArchiver, JsonlStore};
use nanocode_tool_runtime::{ToolExposure, Tools};
use serde::Deserialize;

mod markdown;
mod tui;

const PROMPT: &str = include_str!("../prompt.md");
const DEFAULT_MODEL: &str = "luna";

type Defaults = nanocode_agent_host::Defaults<CodexHarness, CodexHarness>;
type Nanocode = nanocode_agent::Runtime<Defaults>;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const HELP: &str = "nc-lite: nanocode with one harness and plain tools
usage: nc-lite [--continue] [--ephemeral] [--home DIR] [--base-url URL] [-m MODEL]
       nc-lite exec [--continue] [--ephemeral] [--home DIR] [--base-url URL] [-m MODEL] PROMPT
       nc-lite sessions [--home DIR]
       nc-lite --version";

enum Command {
    Interactive,
    Exec,
    Sessions,
    Version,
    Help,
}

#[derive(Clone)]
struct Config {
    home: PathBuf,
    base_url: Option<String>,
    model: String,
    ephemeral: bool,
    continue_last: bool,
}

fn args() -> Result<(Command, Config, Option<String>)> {
    let mut it = std::env::args().skip(1).peekable();
    let command = match it.peek().map(String::as_str) {
        None => Command::Interactive,
        Some("exec") => Command::Exec,
        Some("sessions") => Command::Sessions,
        Some("--version" | "-V") => Command::Version,
        Some(flag) if flag.starts_with('-') && !matches!(flag, "-h" | "--help") => {
            Command::Interactive
        }
        Some(_) => Command::Help,
    };
    if matches!(command, Command::Exec | Command::Sessions) {
        it.next();
    }
    let mut config = Config {
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set")?
            .join(".nc-lite"),
        base_url: None,
        model: DEFAULT_MODEL.into(),
        ephemeral: false,
        continue_last: false,
    };
    let mut prompt = None;
    if matches!(command, Command::Version | Command::Help) {
        return Ok((command, config, prompt));
    }
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--home" => config.home = it.next().ok_or("--home needs a value")?.into(),
            "--base-url" => config.base_url = Some(it.next().ok_or("--base-url needs a value")?),
            "-m" | "--model" => config.model = it.next().ok_or("-m needs a value")?,
            "--ephemeral" => config.ephemeral = true,
            "-c" | "--continue" => config.continue_last = true,
            _ => prompt = Some(arg),
        }
    }
    if matches!(command, Command::Interactive)
        && let Some(extra) = prompt
    {
        return Err(format!("unexpected argument {extra}; use nc-lite exec PROMPT").into());
    }
    Ok((command, config, prompt))
}

fn main() -> std::process::ExitCode {
    #[cfg(unix)]
    if let Some(code) = nanocode_psh::serve_if_requested(|arguments| psh::run_with(arguments, &[]))
    {
        return code;
    }
    #[cfg(unix)]
    {
        let chosen = nanocode_psh::shell().map(nanocode_home::shell::choose);
        if !matches!(chosen, Ok(Ok(()))) || nanocode_psh_embedded::embed().is_err() {
            eprintln!("nc-lite: failed to make psh the shell: {chosen:?}");
            return std::process::ExitCode::FAILURE;
        }
    }
    #[cfg(not(unix))]
    {
        eprintln!("nc-lite: psh, the shell of its commands, is built for Unix hosts only");
        return std::process::ExitCode::FAILURE;
    }
    let outcome = args().and_then(|(command, config, prompt)| match command {
        Command::Version => {
            println!("nc-lite {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Help => {
            println!("{HELP}");
            Ok(())
        }
        Command::Interactive => tui::run(config),
        Command::Exec => tokio_runtime()?.block_on(exec(config, prompt)),
        Command::Sessions => tokio_runtime()?.block_on(sessions(&config)),
    });
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nc-lite: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn tokio_runtime() -> Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.worker_threads(1).enable_all();
    Ok(builder.build()?)
}

struct Lite {
    runtime: Nanocode,
    store: Option<SessionStore>,
    archiver: Option<JsonlArchiver>,
    model: Model,
}

impl Lite {
    async fn start(config: Config) -> Result<Self> {
        let key =
            std::env::var("AI_GATEWAY_API_KEY").map_err(|_| "AI_GATEWAY_API_KEY is not set")?;
        let mut gateway = ResponsesGateway::vercel();
        if let Some(base) = config.base_url {
            gateway = gateway.base_url(base);
        }
        let model: Model = config.model.parse()?;
        let openai = OpenAi::builder(OpenAiAuth::api_key(key))
            .responses_gateway(gateway)
            .model(model)
            .build()?;
        let tools = Tools::builder()
            .exposure(ToolExposure::DirectOnly)
            .web_search(false)
            .image_generation(false)
            .build()?;
        let (store, archiver) = if config.ephemeral {
            (None, None)
        } else {
            let (store, archiver) = open_store(&config.home).await?;
            (Some(store), Some(archiver))
        };
        let builder = Nanocode::builder()
            .openai(openai)
            .tools(tools)
            .without_cua()
            .runner_management(false)
            .instructions(PROMPT);
        let builder = match &store {
            Some(store) => builder.durability(store.clone()),
            None => builder.durability(MemoryStore::new()?),
        };
        let runtime = builder.build_async().await?;
        Ok(Self {
            runtime,
            store,
            archiver,
            model,
        })
    }

    async fn session(&self, continue_last: bool) -> Result<(Agent, AgentEvents)> {
        match (&self.store, continue_last) {
            (Some(store), true) => {
                let latest = workspace_sessions(store)
                    .await?
                    .into_iter()
                    .next()
                    .ok_or("no saved session for this workspace")?;
                Ok(self.runtime.open_session(&latest.record.session_id).await?)
            }
            (None, true) => Err("--continue needs a store; remove --ephemeral".into()),
            _ => Ok(self
                .runtime
                .spawn()
                .backend::<Codex>()
                .model(self.model)
                .build()
                .await?),
        }
    }

    async fn shutdown(self) -> Result<()> {
        self.runtime.shutdown().await?;
        if let (Some(store), Some(archiver)) = (&self.store, self.archiver) {
            store.summarize(&archiver.written().await?).await?;
            archiver.archive().await?;
        }
        Ok(())
    }
}

async fn open_store(home: &std::path::Path) -> Result<(SessionStore, JsonlArchiver)> {
    let store = JsonlStore::open_home_async(home).await?;
    let archiver = store.archiver();
    Ok((
        nanocode_agent::session_store::<Defaults, _>(store)?,
        archiver,
    ))
}

async fn workspace_sessions(store: &SessionStore) -> Result<Vec<SessionSummary>> {
    let workspace = std::env::current_dir()?;
    Ok(store
        .list()
        .await?
        .into_iter()
        .filter(|s| s.record.workspace.as_deref() == Some(workspace.as_path()))
        .collect())
}

async fn sessions(config: &Config) -> Result<()> {
    let (store, archiver) = open_store(&config.home).await?;
    let listed = workspace_sessions(&store).await?;
    let mut out = std::io::stdout().lock();
    for session in listed {
        writeln!(
            out,
            "{}  {}",
            session.record.session_id,
            session.preview.unwrap_or_default()
        )?;
    }
    drop(out);
    archiver.archive().await?;
    Ok(())
}

async fn exec(config: Config, prompt: Option<String>) -> Result<()> {
    let prompt = prompt
        .filter(|p| !p.trim().is_empty())
        .ok_or("exec needs a prompt")?;
    let continue_last = config.continue_last;
    let lite = Lite::start(config).await?;
    let outcome: Result<()> = async {
        let (agent, mut events) = lite.session(continue_last).await?;
        let mut result = std::pin::pin!(agent.prompt(prompt.as_str()).await?.result());
        let mut out = std::io::BufWriter::new(std::io::stdout().lock());
        let mut streamed = false;
        let result = loop {
            while let Some(event) = events.try_recv() {
                streamed |= write_event(&mut out, &event)?;
            }
            out.flush()?;
            tokio::select! {
                biased;
                Some(event) = events.recv() => streamed |= write_event(&mut out, &event)?,
                result = &mut result => break result?,
            }
        };
        while let Some(event) = events.try_recv() {
            streamed |= write_event(&mut out, &event)?;
        }
        if !streamed {
            writeln!(out, "{}", result.final_message())?;
        } else {
            writeln!(out)?;
        }
        out.flush()?;
        eprintln!("session {}", agent.session_id());
        agent.flush().await?;
        Ok(())
    }
    .await;
    let shutdown = lite.shutdown().await;
    outcome?;
    shutdown?;
    Ok(())
}

enum Shown<'a> {
    Text(Cow<'a, str>),
    Tool(Cow<'a, str>),
}

#[derive(Default, Deserialize)]
struct Payload<'a> {
    #[serde(borrow)]
    text: Option<Cow<'a, str>>,
    #[serde(borrow)]
    tool: Option<Cow<'a, str>>,
}

fn shown(event: &AgentEvent) -> Option<Shown<'_>> {
    if !matches!(
        event.kind,
        AgentEventKind::AssistantDelta | AgentEventKind::ToolCall
    ) {
        return None;
    }
    let payload = serde_json::from_str::<Payload<'_>>(event.payload.get()).unwrap_or_default();
    match event.kind {
        AgentEventKind::AssistantDelta => Some(Shown::Text(payload.text.unwrap_or_default())),
        _ => Some(Shown::Tool(payload.tool.unwrap_or(Cow::Borrowed("tool")))),
    }
}

fn write_event(out: &mut impl Write, event: &AgentEvent) -> Result<bool> {
    match shown(event) {
        Some(Shown::Text(text)) => {
            out.write_all(text.as_bytes())?;
            Ok(true)
        }
        Some(Shown::Tool(tool)) => {
            eprintln!("· {tool}");
            Ok(false)
        }
        None => Ok(false),
    }
}
