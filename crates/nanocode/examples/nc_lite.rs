use std::io::Write;
use std::path::PathBuf;

use nanocode::agent::events::AgentEventKind;
use nanocode::codex::{auth::OpenAiAuth, transport::ResponsesGateway};
use nanocode::{JsonlStore, Model, Nanocode, OpenAi, Tools};
use nanocode_tool_runtime::ToolExposure;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const HELP: &str = "nc-lite: nanocode with one harness and plain tools
usage: nc-lite exec [--continue] [--ephemeral] [--home DIR] [--base-url URL] [-m MODEL] PROMPT
       nc-lite sessions [--home DIR]
       nc-lite --version";

struct Args {
    command: String,
    prompt: Option<String>,
    home: PathBuf,
    base_url: Option<String>,
    model: String,
    ephemeral: bool,
    continue_last: bool,
}

fn args() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let command = it.next().unwrap_or_default();
    let mut parsed = Args {
        command,
        prompt: None,
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set")?
            .join(".nc-lite"),
        base_url: None,
        model: "luna".into(),
        ephemeral: false,
        continue_last: false,
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--home" => parsed.home = it.next().ok_or("--home needs a value")?.into(),
            "--base-url" => parsed.base_url = Some(it.next().ok_or("--base-url needs a value")?),
            "-m" | "--model" => parsed.model = it.next().ok_or("-m needs a value")?,
            "--ephemeral" => parsed.ephemeral = true,
            "-c" | "--continue" => parsed.continue_last = true,
            _ => parsed.prompt = Some(arg),
        }
    }
    Ok(parsed)
}

fn main() -> std::process::ExitCode {
    let first = std::env::args().nth(1);
    match first.as_deref() {
        Some("--version" | "-V") => {
            println!("nc-lite {}", env!("CARGO_PKG_VERSION"));
            return std::process::ExitCode::SUCCESS;
        }
        Some("exec" | "sessions") => {}
        _ => {
            println!("{HELP}");
            return std::process::ExitCode::SUCCESS;
        }
    }
    let outcome = args().and_then(|args| {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.worker_threads(1).enable_all();
        let runtime = builder.build()?;
        runtime.block_on(async move {
            if args.command == "sessions" {
                sessions(&args).await
            } else {
                exec(args).await
            }
        })
    });
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nc-lite: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn sessions(args: &Args) -> Result<()> {
    let store = nanocode::session_store(JsonlStore::open_home_async(&args.home).await?)?;
    let workspace = std::env::current_dir()?;
    let mut out = std::io::stdout().lock();
    for session in store.list().await? {
        if session.record.workspace.as_deref() == Some(workspace.as_path()) {
            writeln!(
                out,
                "{}  {}",
                session.record.session_id,
                session.preview.unwrap_or_default()
            )?;
        }
    }
    Ok(())
}

async fn exec(args: Args) -> Result<()> {
    let prompt = args
        .prompt
        .clone()
        .filter(|p| !p.trim().is_empty())
        .ok_or("exec needs a prompt")?;
    let key = std::env::var("AI_GATEWAY_API_KEY").map_err(|_| "AI_GATEWAY_API_KEY is not set")?;
    let mut gateway = ResponsesGateway::vercel();
    if let Some(base) = &args.base_url {
        gateway = gateway.base_url(base.clone());
    }
    let model: Model = args.model.parse()?;
    let openai = OpenAi::builder(OpenAiAuth::api_key(key))
        .responses_gateway(gateway)
        .model(model)
        .build()?;
    let tools = Tools::builder()
        .exposure(ToolExposure::DirectOnly)
        .web_search(false)
        .image_generation(false)
        .build()?;
    let store = if args.ephemeral {
        None
    } else {
        Some(nanocode::session_store(
            JsonlStore::open_home_async(&args.home).await?,
        )?)
    };
    let mut builder = Nanocode::builder()
        .openai(openai)
        .tools(tools)
        .without_cua()
        .runner_management(false);
    builder = match &store {
        Some(store) => builder.durability(store.clone()),
        None => builder.durability(nanocode::durability::MemoryStore::new()?),
    };
    let runtime = builder.build_async().await?;
    let outcome: Result<()> = async {
        let (agent, events) = match (&store, args.continue_last) {
            (Some(store), true) => {
                let workspace = std::env::current_dir()?;
                let latest = store
                    .list()
                    .await?
                    .into_iter()
                    .find(|s| s.record.workspace.as_deref() == Some(workspace.as_path()))
                    .ok_or("no saved session for this workspace")?;
                runtime.open_session(&latest.record.session_id).await?
            }
            (None, true) => return Err("--continue needs a store; remove --ephemeral".into()),
            _ => {
                runtime
                    .spawn()
                    .backend::<nanocode::Codex>()
                    .model(model)
                    .build()
                    .await?
            }
        };
        let mut events = events;
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
    let shutdown = runtime.shutdown().await;
    outcome?;
    shutdown?;
    Ok(())
}

fn write_event(out: &mut impl Write, event: &nanocode::agent::events::AgentEvent) -> Result<bool> {
    match event.kind {
        AgentEventKind::AssistantDelta => {
            let payload: serde_json::Value =
                serde_json::from_str(event.payload.get()).unwrap_or_default();
            out.write_all(payload["text"].as_str().unwrap_or_default().as_bytes())?;
            Ok(true)
        }
        AgentEventKind::ToolCall => {
            let payload: serde_json::Value =
                serde_json::from_str(event.payload.get()).unwrap_or_default();
            eprintln!("· {}", payload["tool"].as_str().unwrap_or("tool"));
            Ok(false)
        }
        _ => Ok(false),
    }
}
