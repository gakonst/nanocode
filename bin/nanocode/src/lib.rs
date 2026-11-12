#![cfg(not(target_family = "wasm"))]

mod actions;
mod app;
#[cfg(target_os = "linux")]
mod arguments;
mod cli;
mod commands;
#[cfg(unix)]
mod cua;
mod custom_endpoint;
mod events;
mod extras;
mod focus;
mod history;
mod host;
mod login;
mod models;
mod sessions;
mod side;
mod split;
mod stream;
mod telemetry;
mod tempo;
mod view;

use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Parser;
use futures_util::StreamExt;
use nanocode::{ModelSelection, ServiceTier, Thinking, durability::TranscriptItem};

use crate::{
    cli::{Cli, Command, GatewayArgs, HandArgs, HarnessArg, SessionArgs, SessionsArgs},
    host::{Host, HostConfig, Result},
};

pub use host::{ModelClients, Panel};

const TELEMETRY_SHUTDOWN: std::time::Duration = std::time::Duration::from_millis(500);

fn session_gateway(cli: &Cli) -> Option<GatewayArgs> {
    match &cli.command {
        None | Some(Command::Models | Command::Resume { .. }) => Some(cli.session.gateway.clone()),
        Some(Command::Exec(args)) => Some(args.clone().or(cli.session.clone()).gateway),
        _ => None,
    }
}

fn cli_home(cli: &Cli) -> Result<PathBuf> {
    let explicit_home = match &cli.command {
        Some(Command::Exec(args)) => args.home.as_deref(),
        Some(Command::Sessions(args)) => args.home.as_deref(),
        _ => None,
    }
    .or(cli.session.home.as_deref());
    host::home_dir(explicit_home)
}

fn install_telemetry(
    cli: &Cli,
    home: &Path,
) -> Result<Option<nanocode_observability::ObservabilityGuard>> {
    let args = &cli.observability;
    let interactive = !matches!(
        cli.command,
        Some(Command::Exec(_) | Command::Hand(_) | Command::Sessions(_) | Command::Models)
    ) && !is_cua(cli);
    let output = match (args.log_filter().is_some(), interactive) {
        (false, _) => nanocode_observability::LogOutput::Disabled,
        (true, false) => nanocode_observability::LogOutput::Stderr,
        (true, true) => {
            nanocode_observability::LogOutput::File(home.join("log").join("nanocode.log"))
        }
    };
    Ok(Some(
        args.builder(
            "nanocode",
            env!("CARGO_PKG_VERSION"),
            output,
            if matches!(cli.command, Some(Command::Hand(_))) {
                nanocode_observability::Telemetry::Off
            } else {
                nanocode_observability::Telemetry::Agent
            },
        )
        .otlp_directory(home.join("log").join("otel"))
        .shutdown_timeout(TELEMETRY_SHUTDOWN)
        .install()?,
    ))
}

#[cfg(unix)]
const fn is_cua(cli: &Cli) -> bool {
    matches!(cli.command, Some(Command::Cua(_)))
}

#[cfg(not(unix))]
const fn is_cua(_: &Cli) -> bool {
    false
}

pub fn run() -> ExitCode {
    run_with(ModelClients::default())
}

pub fn run_with(clients: ModelClients) -> ExitCode {
    #[cfg(unix)]
    if let Some(code) = nanocode_psh::serve_if_requested(|arguments| {
        psh::run_with(arguments, &[nanocode_python::UTILITY])
    }) {
        return code;
    }
    #[cfg(unix)]
    if let Some(code) = nanocode_process::read_outputs_if_requested() {
        return code;
    }
    let entered = std::time::Instant::now();
    let cli = Cli::parse();
    #[cfg(unix)]
    let shell = nanocode_psh::shell();
    #[cfg(target_os = "linux")]
    if (cli.session.prompt.is_some()
        || matches!(&cli.command, Some(cli::Command::Exec(args)) if args.prompt.is_some()))
        && let Err(error) = arguments::hide()
    {
        eprintln!("nanocode: the prompt stays visible in this process's arguments: {error}");
    }
    let parsed = entered.elapsed();
    #[cfg(unix)]
    {
        let chosen = shell.map(nanocode_home::shell::choose);
        let embeds =
            !matches!(&cli.command, Some(cli::Command::Hand(args)) if args.own_processes);
        if !matches!(chosen, Ok(Ok(()))) || (embeds && nanocode_psh_embedded::embed().is_err()) {
            eprintln!("nanocode: failed to make psh the shell: {chosen:?}");
            return ExitCode::FAILURE;
        }
    }
    #[cfg(not(unix))]
    {
        eprintln!("nanocode: psh, the shell of its commands, is built for Unix hosts only");
        return ExitCode::FAILURE;
    }
    let clients = match custom_endpoint::apply(clients) {
        Ok(clients) => clients,
        Err(error) => {
            eprintln!("nanocode: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (clients, closing) = match session_gateway(&cli) {
        Some(gateway) => match tempo::apply(clients, &gateway) {
            Ok(applied) => applied,
            Err(error) => {
                eprintln!("nanocode: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => (clients, None),
    };
    let home = match cli_home(&cli) {
        Ok(home) => home,
        Err(error) => {
            eprintln!("nanocode: failed to resolve the home directory: {error}");
            return ExitCode::FAILURE;
        }
    };
    let telemetry_started = std::time::Instant::now();
    let telemetry = match install_telemetry(&cli, &home) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("nanocode: failed to start telemetry: {error}");
            return ExitCode::FAILURE;
        }
    };
    let startup = tracing::info_span!(
        target: "nanocode",
        "nc.startup",
        process.pid = std::process::id(),
        cli.duration_ms = parsed.as_secs_f64() * 1_000.0,
        telemetry.duration_ms = telemetry_started.elapsed().as_secs_f64() * 1_000.0,
        main_to_span.duration_ms = entered.elapsed().as_secs_f64() * 1_000.0,
        runtime.duration_ms = tracing::field::Empty
    );
    let runtime_started = std::time::Instant::now();
    let headless = matches!(cli.command, Some(Command::Exec(_) | Command::Sessions(_)));
    let runtime = match startup.in_scope(|| {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        if headless {
            builder.worker_threads(1);
        }
        builder.enable_all().build()
    }) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("nanocode: failed to start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    startup.record(
        "runtime.duration_ms",
        runtime_started.elapsed().as_secs_f64() * 1_000.0,
    );
    drop(startup);
    if matches!(
        cli.command,
        None | Some(Command::Exec(_) | Command::Resume { .. })
    ) && let Some(warming) = clients.warming()
    {
        runtime.spawn(warming);
    }
    let outcome = runtime.block_on(async move {
        match cli.command {
            Some(Command::Hand(args)) => hand(args).await,
            #[cfg(unix)]
            Some(Command::Cua(args)) => cua::cua(args).await,
            Some(Command::Sessions(mut args)) => {
                args.home = args.home.or(cli.session.home);
                on_worker(list_sessions(args)).await
            }
            Some(Command::Exec(args)) => on_worker(exec(args.or(cli.session), clients)).await,
            Some(Command::Models) => {
                print_models(&clients).await;
                Ok(())
            }
            Some(Command::Resume { id }) => {
                let mut args = cli.session;
                args.resume = Some(id.unwrap_or_default());
                interactive(args, clients).await
            }
            None => interactive(cli.session, clients).await,
        }
    });
    if let Some(closing) = closing {
        runtime.block_on(tempo::close(closing));
    }
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    drop(telemetry);
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nanocode: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn on_worker(work: impl Future<Output = Result<()>> + Send + 'static) -> Result<()> {
    tokio::spawn(work).await?
}

struct Prepared {
    host: Host,
    model: ModelSelection,
    route: host::Route,
    thinking: Thinking,
    tier: ServiceTier,
    home: PathBuf,
}

async fn prepare(
    args: &SessionArgs,
    clients: ModelClients,
    notify: &mut (dyn FnMut(&str) + Send),
    headless: bool,
) -> Result<Prepared> {
    let workspace = match &args.workspace {
        Some(path) => std::fs::canonicalize(path)
            .map_err(|error| format!("workspace {}: {error}", path.display()))?,
        None => std::env::current_dir()?,
    };
    let home = host::home_dir(args.home.as_deref())?;
    let gateway = host::gateway(&args.gateway)?;
    if args.cyber_access_program.is_some() && gateway.is_some() {
        return Err(
            "--cyber-access-program needs OpenAI itself; it is unavailable with --gateway".into(),
        );
    }
    if gateway.is_some() && clients.supplies_openai() {
        return Err("this binary supplies its own Codex client; --gateway is unavailable".into());
    }
    let (route, model) = match (&args.model, args.harness) {
        (Some(model), harness) => {
            let (route, model) = clients.resolve(model)?;
            let model = match harness {
                Some(harness) if host::follows_harness(route, &model) => {
                    host::on_harness(&model, &family(harness))
                }
                _ => model,
            };
            if let Some(harness) = harness
                && family(harness) != model.family()
            {
                return Err(format!(
                    "model {} does not belong to the {} harness",
                    route.label(&model),
                    family(harness)
                )
                .into());
            }
            (route, model)
        }
        (None, harness) => {
            let requested = harness.map(family);
            let model = match clients.codex_models().and_then(<[_]>::first) {
                Some(model)
                    if requested
                        .as_ref()
                        .is_none_or(|family| *family == nanocode::HarnessFamily::Codex) =>
                {
                    nanocode::ModelSelection::Codex(*model)
                }
                _ => requested
                    .unwrap_or(nanocode::HarnessFamily::Claude)
                    .default_model(),
            };
            (host::Route::Native, model)
        }
    };
    if let Some(gateway) = &gateway
        && route == host::Route::Native
    {
        gateway.check(&model)?;
    }
    let defaulted =
        args.model.is_none() && args.harness.is_none() && clients.codex_models().is_none();
    let capabilities = model.capabilities(nanocode::ModelTransport::Native);
    let thinking = match &args.thinking {
        Some(value) => host::parse_thinking(value)?,
        None if defaulted && capabilities.supports_thinking(Thinking::Xhigh) => Thinking::Xhigh,
        None => model.default_thinking(),
    };
    model
        .capabilities(nanocode::ModelTransport::Native)
        .check_thinking(thinking)?;
    let tier = match &args.tier {
        Some(value) => host::parse_tier(value)?,
        None => ServiceTier::Standard,
    };
    model
        .capabilities(nanocode::ModelTransport::Native)
        .check_service_tier(tier)?;
    let (store, archiver) = if args.ephemeral {
        (None, None)
    } else {
        let (store, archiver) = host::open_store(&home).await?;
        (Some(store), Some(archiver))
    };
    let runner_configs = host::runner_configs(&args.remote)?;
    let config = HostConfig {
        workspace,
        store,
        archiver,
        subagents: !args.no_subagents,
        runners: runner_configs.runners,
        runner_servers: runner_configs.servers,
        tab: runner_configs.tab,
        gateway,
        clients,
        headless,
        cyber_access_program: args.cyber_access_program.map(Into::into),
    };
    let host = tracing::Instrument::instrument(
        Host::build(config, &model, thinking),
        tracing::info_span!(target: "nanocode", "nc.host.build"),
    )
    .await?;
    tracing::Instrument::instrument(
        host.runtime.initialize_runners(),
        tracing::info_span!(target: "nanocode", "nc.runners.initialize"),
    )
    .await?;
    for server in host.runtime.runner_servers() {
        notify(&host::listener_notice(&server));
    }
    if let Some(link) = &host.config.tab {
        notify(&host::tab_notice(link));
    }
    Ok(Prepared {
        host,
        model,
        route,
        thinking,
        tier,
        home,
    })
}

const fn family(harness: HarnessArg) -> nanocode::HarnessFamily {
    match harness {
        HarnessArg::Codex => nanocode::HarnessFamily::Codex,
        HarnessArg::Claude => nanocode::HarnessFamily::Claude,
    }
}

async fn interactive(mut args: SessionArgs, clients: ModelClients) -> Result<()> {
    let reload = match &args.reload_state {
        Some(path) => Some(app::reload::read(path).await?),
        None => None,
    };
    if let Some(state) = &reload {
        args.resume = if state.moved {
            Some(String::new())
        } else {
            state.session.clone()
        };
        args.prompt = None;
        args.continue_last = false;
        args.model = Some(host::Route::of(state.provider.as_deref())?.label(&state.model));
        args.harness = None;
        args.thinking = Some(state.thinking.to_string());
        args.tier = Some(state.tier.as_str().to_owned());
    }
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return Err(
            "the interactive interface needs a terminal; use `nanocode exec <prompt>` for scripts"
                .into(),
        );
    }
    let terminal =
        nanocode_tui::terminal::TerminalSession::enter(nanocode_tui::terminal::TerminalOptions {
            mouse_capture: !args.no_mouse,
            keyboard_enhancement: true,
        })?;
    let prepared = prepare(&args, clients, &mut |_| {}, false).await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            drop(terminal);
            return Err(error);
        }
    };
    let (resume, pick_session) = match args.resume.as_deref() {
        Some("") => (None, true),
        Some(id) => (Some(id.to_owned()), false),
        None if args.continue_last => match latest_session(&prepared.host).await {
            Ok(id) => (Some(id), false),
            Err(error) => {
                drop(terminal);
                return Err(error);
            }
        },
        None => (None, false),
    };
    if (resume.is_some() || pick_session) && prepared.host.config.store.is_none() {
        drop(terminal);
        return Err("--resume and --continue require durability; remove --ephemeral".into());
    }
    app::run(
        app::Launch {
            host: prepared.host,
            model: prepared.model,
            route: prepared.route,
            thinking: prepared.thinking,
            tier: prepared.tier,
            resume,
            pick_session,
            prompt: args.prompt,
            history_path: Some(prepared.home.join("history.jsonl")),
            home: prepared.home.clone(),
            thinking_override: args
                .thinking
                .as_deref()
                .map(host::parse_thinking)
                .transpose()?,
            tier_override: args.tier.as_deref().map(host::parse_tier).transpose()?,
            reload,
            reload_path: args.reload_state,
        },
        terminal,
    )
    .await
}

async fn latest_session(host: &Host) -> Result<String> {
    let store = host
        .config
        .store
        .as_ref()
        .ok_or("--continue requires durability")?;
    store
        .list()
        .await?
        .into_iter()
        .find(|session| {
            sessions::user_facing(session.record.lineage.origin)
                && session.record.workspace.as_deref() == Some(host.config.workspace.as_path())
        })
        .map(|session| session.record.session_id)
        .ok_or_else(|| "no saved session for this workspace".into())
}

async fn exec(args: SessionArgs, clients: ModelClients) -> Result<()> {
    let prompt = args
        .prompt
        .clone()
        .filter(|prompt| !prompt.trim().is_empty())
        .ok_or("exec requires a prompt")?;
    let prepared = prepare(
        &args,
        clients,
        &mut |message| {
            eprintln!("nanocode: {message}");
        },
        true,
    )
    .await?;
    let host = prepared.host;
    let resume = match (&args.resume, args.continue_last) {
        (Some(id), _) if !id.is_empty() => Some(id.clone()),
        (_, true) => Some(latest_session(&host).await?),
        _ => None,
    };
    let (agent, events, pending) = match resume {
        Some(id) => {
            let ready = app::open_session(
                &host,
                &id,
                args.thinking
                    .as_deref()
                    .map(host::parse_thinking)
                    .transpose()?,
                args.tier.as_deref().map(host::parse_tier).transpose()?,
            )
            .await?;
            let (asked, runs) = (
                prepared.route.label(&prepared.model),
                ready.route.label(&ready.model),
            );
            if args.model.is_some() && asked != runs {
                drop(ready.agent.shutdown().await);
                return Err(format!(
                    "session {id} runs {runs}, and a session keeps its model; resume it without --model {asked}, or start a new session"
                )
                .into());
            }
            (ready.agent, ready.events, ready.pending)
        }
        None => {
            let (agent, events) = host
                .start(&prepared.model, prepared.thinking, prepared.route)
                .await?;
            app::apply_tier(&agent, &prepared.model, prepared.tier).await?;
            (agent, events, Vec::new())
        }
    };
    if let Some(reason) = host.runtime.computer_use_unavailable() {
        eprintln!("nanocode: computer use unavailable, continuing without it: {reason}");
    }
    drop(events);
    if let Some(mut notices) = host.config.clients.notifying() {
        tokio::spawn(async move {
            while let Some(notice) = notices.next().await {
                eprintln!("{notice}");
            }
        });
    }
    let outcome: Result<()> = async {
        for pending in pending {
            eprintln!("nanocode: resuming interrupted turn {}", pending.id);
            let turn = agent
                .prompt(nanocode::PromptRequest::new(pending.prompt).request_id(&pending.id))
                .await?;
            stream_turn(turn, std::io::stderr()).await?;
        }
        stream_turn(agent.prompt(prompt.as_str()).await?, std::io::stdout()).await
    }
    .await;
    let mut stderr = std::io::stderr();
    if let Some(status) = host.config.clients.describe() {
        drop(writeln!(stderr, "{status}"));
    }
    if let Some(panel) = host.config.clients.render_panel() {
        drop(writeln!(stderr, "{}", panel.title));
        for row in panel.aligned() {
            drop(writeln!(stderr, "  {}", row.join("  ").trim_end()));
        }
    }
    drop(writeln!(stderr, "session {}", agent.session_id()));
    let flushed = agent.flush().await;
    let shutdown = host.runtime.shutdown().await;
    let archived = host::archive(&host.config).await;
    outcome?;
    flushed?;
    shutdown?;
    archived?;
    Ok(())
}

async fn stream_turn(mut turn: nanocode::agent::Turn, answers: impl Write) -> Result<()> {
    let mut printer = stream::Printer::new(std::io::BufWriter::new(answers));
    loop {
        while let Some(event) = turn.try_next() {
            printer.event(&event)?;
        }
        printer.flush()?;
        let Some(event) = turn.next().await else {
            break;
        };
        printer.event(&event)?;
    }
    let result = turn.await?;
    printer.finish(result.final_message())?;
    Ok(())
}

async fn list_sessions(args: SessionsArgs) -> Result<()> {
    let home = host::home_dir(args.home.as_deref())?;
    let (store, _) = host::open_store(&home).await?;
    if let Some(id) = &args.session {
        for item in store.load(id).await?.transcript {
            match item {
                TranscriptItem::User(text) => println!("## user\n{text}\n"),
                TranscriptItem::Assistant(text) => println!("## assistant\n{text}\n"),
                _ => {}
            }
        }
        store.flush().await?;
        return Ok(());
    }
    let workspace = std::env::current_dir()?;
    for session in store.list().await? {
        let record = &session.record;
        if !sessions::user_facing(record.lineage.origin) {
            continue;
        }
        if !args.all && record.workspace.as_deref() != Some(workspace.as_path()) {
            continue;
        }
        println!(
            "{}  {:>8}  {:<20}  {}",
            record.session_id,
            sessions::age(record.updated_at_ms),
            sessions::model(record),
            sessions::title(&session)
        );
    }
    store.flush().await?;
    Ok(())
}

async fn print_models(clients: &ModelClients) {
    if let Some(tempo) = clients.tempo_models() {
        tempo.load().await;
    }
    let groups = models::catalog(clients);
    let width = groups
        .iter()
        .flat_map(|group| &group.models)
        .map(|(route, model, _)| route.label(model).chars().count())
        .max()
        .unwrap_or_default();
    for group in groups {
        println!("{}", group.title);
        if let Some(note) = &group.note {
            println!("  {note}");
        }
        for (route, model, about) in &group.models {
            let capabilities = model.capabilities(nanocode::ModelTransport::Native);
            let thinking = capabilities
                .thinking()
                .map(Thinking::as_str)
                .collect::<Vec<_>>()
                .join("/");
            let features = models::features(model);
            let detail = [features.as_str(), about.as_deref().unwrap_or_default()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            println!(
                "  {:<width$} thinking {:<30} default {:<7} {}",
                route.label(model),
                thinking,
                capabilities.default_thinking().as_str(),
                detail
            );
        }
    }
}

async fn hand(args: HandArgs) -> Result<()> {
    use nanocode::hand::{Hand, HandEvent};
    let directory = match &args.directory {
        Some(path) => std::fs::canonicalize(path)?,
        None => std::env::current_dir()?,
    };
    let mut tools = nanocode::hand::tools(directory.clone())?.into_builder();
    if args.cua {
        let computer = nanocode_computer_repl::ComputerTools::connect(
            nanocode_computer_repl::ComputerConfig::detect()?,
        )
        .await?;
        tools = tools.tool(computer.js()).tool(computer.reset());
    }
    if let Some(after) = args.cell_yield_ms {
        tools = tools.cell_yield(std::time::Duration::from_millis(after));
    }
    let tools = tools.build()?;
    let name = match args.name {
        Some(name) => name,
        None => Hand::machine_name()?,
    };
    let hand = match (args.url, args.listen) {
        (Some(url), _) => Hand::connect(url, args.token, tools),
        (None, Some(address)) => Hand::listen(address, args.token, tools),
        (None, None) => return Err("pass the brain URL or --listen <ADDR>".into()),
    }
    .name(&name)
    .directory(directory.clone());
    let hand = match &args.pricing {
        Some(path) => hand.pricing(nanocode::hand::HandPricing::rules(
            &std::fs::read_to_string(path)?,
        )?),
        None => hand,
    };
    let hand = if args.tools_only {
        hand.tools_only()
    } else {
        hand
    };
    eprintln!("nanocode hand {name} in {}", directory.display());
    let run = hand.run(|event| match event {
        HandEvent::Connecting { endpoint } => eprintln!("hand: connecting {endpoint}"),
        HandEvent::Connected { endpoint, .. } => eprintln!("hand: connected to {endpoint}"),
        HandEvent::Listening { endpoint } => eprintln!("hand: listening {endpoint}"),
        HandEvent::BrainConnected { peer } => eprintln!("hand: brain connected from {peer}"),
        HandEvent::Disconnected { reason } => eprintln!("hand: disconnected ({reason})"),
        HandEvent::Retrying { delay } => eprintln!("hand: retrying in {}ms", delay.as_millis()),
        HandEvent::Revoked => eprintln!("hand: the brain disconnected this hand; stopping"),
        HandEvent::Replaced => eprintln!("hand: another hand attached under this name; stopping"),
        other => eprintln!("hand: {other:?}"),
    });
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => {
            let left = leave_reader();
            result?;
            left?;
        }
        signal = tokio::signal::ctrl_c() => {
            signal?;
            leave_reader()?;
        }
        signal = terminated() => {
            signal?;
            eprintln!("hand: stopped; what its commands left running is read until it ends");
            hand_over()?;
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn terminated() -> std::io::Result<()> {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
        .recv()
        .await;
    Ok(())
}

#[cfg(not(unix))]
async fn terminated() -> std::io::Result<()> {
    std::future::pending().await
}

#[cfg(unix)]
fn reader() -> std::io::Result<std::process::Command> {
    let (program, arguments) = nanocode_psh::program()?;
    let mut command = std::process::Command::new(program);
    command.args(arguments);
    Ok(command)
}

#[cfg(unix)]
fn hand_over() -> std::io::Result<()> {
    std::process::exit(nanocode_process::hand_over(reader()?)?.into())
}

#[cfg(not(unix))]
const fn hand_over() -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn leave_reader() -> std::io::Result<()> {
    if nanocode_process::leave_reader(reader()?)? {
        eprintln!("hand: ended; what its commands left running is read by a process of its own until it ends");
    }
    Ok(())
}

#[cfg(not(unix))]
const fn leave_reader() -> std::io::Result<()> {
    Ok(())
}
