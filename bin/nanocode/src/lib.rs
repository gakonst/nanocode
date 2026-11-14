#![cfg(not(target_family = "wasm"))]

mod actions;
mod app;
#[cfg(target_os = "linux")]
mod arguments;
mod cells;
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
#[cfg(unix)]
mod python;
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

use nanocode::{ModelSelection, ServiceTier, Thinking, durability::TranscriptItem};
use nanocode_sync::StreamExt;

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
    match telemetry(cli, home, true).install() {
        Err(nanocode_observability::ObservabilityError::LocalFile(path, error))
            if path.starts_with(home) =>
        {
            eprintln!(
                "nanocode: running without local logs and telemetry: cannot write {}: {error}; set --home or NANOCODE_HOME to a writable directory to keep them",
                path.display()
            );
            Ok(Some(telemetry(cli, home, false).install()?))
        }
        installed => Ok(Some(installed?)),
    }
}

fn telemetry(
    cli: &Cli,
    home: &Path,
    local_files: bool,
) -> nanocode_observability::ObservabilityBuilder {
    let args = &cli.observability;
    let interactive = !matches!(
        cli.command,
        Some(Command::Exec(_) | Command::Hand(_) | Command::Sessions(_) | Command::Models)
    ) && !is_cua(cli);
    let output = match (args.log_filter().is_some(), interactive, local_files) {
        (false, _, _) | (true, true, false) => nanocode_observability::LogOutput::Disabled,
        (true, false, _) => nanocode_observability::LogOutput::Stderr,
        (true, true, true) => {
            nanocode_observability::LogOutput::File(home.join("log").join("nanocode.log"))
        }
    };
    let builder = args
        .builder(
            "nanocode",
            env!("CARGO_PKG_VERSION"),
            output,
            if matches!(cli.command, Some(Command::Hand(_))) {
                nanocode_observability::Telemetry::Off
            } else {
                nanocode_observability::Telemetry::Agent
            },
        )
        .shutdown_timeout(TELEMETRY_SHUTDOWN);
    if local_files {
        builder.otlp_directory(home.join("log").join("otel"))
    } else {
        builder
    }
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
    if let Some(code) =
        nanocode_psh::serve_if_requested(|arguments| psh::run_with(arguments, &[python::UTILITY]))
    {
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
        let embeds = !matches!(&cli.command, Some(cli::Command::Hand(args)) if args.own_processes);
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
    if let Err(error) = cells::apply() {
        eprintln!("nanocode: {error}");
        return ExitCode::FAILURE;
    }
    let clients = match custom_endpoint::apply(clients) {
        Ok(clients) => clients,
        Err(error) => {
            eprintln!("nanocode: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (clients, mut closing) = match session_gateway(&cli) {
        Some(gateway) => match tempo::apply(
            clients,
            &gateway,
            matches!(cli.command, Some(Command::Exec(_))),
        ) {
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
    let startup = nanocode_trace::info_span!(
        target: "nanocode",
        "nc.startup",
        process.pid = std::process::id(),
        cli.duration_ms = parsed.as_secs_f64() * 1_000.0,
        telemetry.duration_ms = telemetry_started.elapsed().as_secs_f64() * 1_000.0,
        main_to_span.duration_ms = entered.elapsed().as_secs_f64() * 1_000.0,
        runtime.duration_ms = nanocode_trace::field::Empty
    );
    let runtime_started = std::time::Instant::now();
    let runtime = match startup.in_scope(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
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
    let warming = matches!(
        cli.command,
        None | Some(Command::Exec(_) | Command::Resume { .. })
    )
    .then(|| clients.warming())
    .flatten();
    let one_shot = matches!(cli.command, Some(Command::Exec(_)));
    let paying = match closing.as_mut().and_then(tempo::Closing::take_start) {
        #[cfg(unix)]
        Some(start) if one_shot => {
            nanocode_process::prepare_commands(start);
            None
        }
        paying => paying,
    };
    let drawn = matches!(cli.command, None | Some(Command::Resume { .. }));
    let threads = telemetry
        .as_ref()
        .map(nanocode_observability::ObservabilityGuard::exporter_threads);
    if !drawn && let Some(threads) = &threads {
        threads.start();
    }
    if warming.is_some() || paying.is_some() || (drawn && threads.is_some()) {
        nanocode_platform::spawn_on(runtime.handle(), async move {
            if drawn {
                app::FIRST_FRAME.wait().await;
            }
            if let Some(threads) = threads {
                threads.start();
            }
            if let Some(start) = paying {
                tempo::apart(start);
            }
            if let Some(warming) = warming {
                warming.await;
            }
        });
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
            Some(Command::Models) => print_models(&clients).await,
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
    if one_shot {
        runtime.shutdown_background();
    } else {
        runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    }
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
    nanocode_platform::spawn_task(work).await?
}

struct Prepared {
    host: Host,
    model: ModelSelection,
    route: host::Route,
    thinking: Thinking,
    tier: ServiceTier,
    home: PathBuf,
    runner_events: nanocode_sync::mpsc::UnboundedReceiver<nanocode::RunnerEvent>,
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
        subagents: match (args.no_subagents, args.subagents) {
            (true, _) => host::Subagents::Off,
            (false, false) => host::Subagents::NativeModels,
            (false, true) => host::Subagents::AllModels,
        },
        runners: runner_configs.runners,
        runner_servers: runner_configs.servers,
        tab: runner_configs.tab,
        gateway,
        clients,
        headless,
        cyber_access_program: args.cyber_access_program.map(Into::into),
        context_window: args.context_window,
        max_output_tokens: args
            .max_output_tokens
            .map(u32::try_from)
            .transpose()
            .map_err(|error| format!("--max-output-tokens: {error}"))?,
        codex_direct: args.codex_orchestration == Some(cli::CodexOrchestrationArg::Direct),
    };
    let host = nanocode_trace::Instrument::instrument(
        Host::build(config, &model, thinking),
        nanocode_trace::info_span!(target: "nanocode", "nc.host.build"),
    )
    .await?;
    let mut runner_events = host.runtime.runner_events();
    let initialized = nanocode_trace::Instrument::instrument(
        host.runtime.initialize_runners(),
        nanocode_trace::info_span!(target: "nanocode", "nc.runners.initialize"),
    )
    .await;
    if let Err(error) = initialized {
        let mut failures = Vec::new();
        while let Ok(event) = runner_events.try_recv() {
            failures.extend(runner_failure(&event));
        }
        return Err(match failures.is_empty() {
            true => error.into(),
            false => failures.join("; ").into(),
        });
    }
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
        runner_events,
    })
}

fn runner_failure(event: &nanocode::RunnerEvent) -> Option<String> {
    match event {
        nanocode::RunnerEvent::Failed { name, error } => Some(format!("runner {name}: {error}")),
        nanocode::RunnerEvent::StatusChanged { .. } => None,
    }
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
            runner_events: prepared.runner_events,
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
        .list_page(Some(host.config.workspace.as_path()), 1, |record| {
            sessions::user_facing(record.lineage.origin)
        })
        .await?
        .into_iter()
        .next()
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
    let mut runner_events = prepared.runner_events;
    nanocode_platform::spawn_task(async move {
        while let Some(event) = runner_events.recv().await {
            if let Some(failure) = runner_failure(&event) {
                eprintln!("nanocode: {failure}");
            }
        }
    });
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
        nanocode_platform::spawn_task(async move {
            while let Some(notice) = notices.next().await {
                eprintln!("{notice}");
            }
        });
    }
    if let Some(answer) = host.config.clients.answering() {
        std::thread::spawn(move || {
            for line in std::io::stdin().lines().map_while(std::io::Result::ok) {
                answer(&line);
            }
            answer("");
        });
    }
    let outcome: Result<()> = async {
        for pending in pending {
            eprintln!("nanocode: resuming interrupted turn {}", pending.id);
            let turn = agent
                .prompt(nanocode::PromptRequest::new(pending.prompt).request_id(&pending.id))
                .await?;
            stream_turn(turn, std::io::stderr(), std::io::stderr().is_terminal()).await?;
        }
        let live = std::io::stdout().is_terminal();
        stream_turn(agent.prompt(prompt.as_str()).await?, std::io::stdout(), live).await
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

async fn stream_turn(
    mut turn: nanocode::agent::Turn,
    answers: impl Write,
    live: bool,
) -> Result<()> {
    let mut printer = stream::Printer::new(std::io::BufWriter::new(answers), live);
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
        let transcript = store.load(id).await?.transcript;
        let printed = print_transcript(&transcript);
        store.flush().await?;
        return Ok(unless_closed(printed)?);
    }
    let workspace = std::env::current_dir()?;
    let limit = args.limit.map_or(Ok(sessions::PAGE), usize::try_from)?;
    let mut page = store
        .list_page(
            (!args.all).then_some(workspace.as_path()),
            limit.saturating_add(1),
            |record| sessions::user_facing(record.lineage.origin),
        )
        .await?;
    let more = page.len() > limit;
    page.truncate(limit);
    let printed = print_sessions(&page);
    if more && printed.is_ok() {
        eprintln!("nanocode: more sessions than the {limit} shown; --limit <n> lists more");
    }
    store.flush().await?;
    Ok(unless_closed(printed)?)
}

fn print_transcript(transcript: &[TranscriptItem]) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    for item in transcript {
        match item {
            TranscriptItem::User(text) => writeln!(out, "## user\n{text}\n")?,
            TranscriptItem::Assistant(text) => writeln!(out, "## assistant\n{text}\n")?,
            _ => {}
        }
    }
    out.flush()
}

fn print_sessions(page: &[nanocode::durability::SessionSummary]) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    for session in page {
        let record = &session.record;
        writeln!(
            out,
            "{}  {:>8}  {:<20}  {}",
            record.session_id,
            sessions::age(record.updated_at_ms),
            sessions::model(record),
            sessions::title(session)
        )?;
    }
    out.flush()
}

fn unless_closed(printed: std::io::Result<()>) -> std::io::Result<()> {
    match printed {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        printed => printed,
    }
}

async fn print_models(clients: &ModelClients) -> Result<()> {
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
    let mut out = std::io::stdout().lock();
    let printed = groups.iter().try_for_each(|group| {
        writeln!(out, "{}", group.title)?;
        if let Some(note) = &group.note {
            writeln!(out, "  {note}")?;
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
            writeln!(
                out,
                "  {:<width$} thinking {:<30} default {:<7} {}",
                route.label(model),
                thinking,
                capabilities.default_thinking().as_str(),
                detail
            )?;
        }
        Ok(())
    });
    Ok(unless_closed(printed.and_then(|()| out.flush()))?)
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
    if let Some(after) = args.cell_yield_ms {
        eprintln!(
            "hand: a cell whose call names no yield_time_ms yields after {after} ms; a brain that names none and gives up on an operation sooner takes the cell for lost: set the brain's cell yield instead"
        );
    }
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
    let mut run = std::pin::pin!(run);
    nanocode_sync::select! {
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
        eprintln!(
            "hand: ended; what its commands left running is read by a process of its own until it ends"
        );
    }
    Ok(())
}

#[cfg(not(unix))]
const fn leave_reader() -> std::io::Result<()> {
    Ok(())
}
