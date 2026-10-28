use std::{
    collections::HashMap,
    path::PathBuf,
    task::Poll,
    time::{Duration, Instant},
};

use clap::Parser;
use futures_util::StreamExt;
use nanocode::{
    AgentStatus, ExecutionPolicyDisposition, ExecutorSelection, HarnessFamily, ModelSelection,
    PromptRequest, PromptRoute, Runner, RunnerEvent, RunnerInfo, RunnerProtocol, RunnerServer,
    RunnerServerConfig, RunnerStatus, ScopedAgentUpdate, ServiceTier, SessionCheckpoint, Thinking,
    TurnControl, TurnResult,
    agent::{
        Agent, AgentEvents,
        events::{AgentEvent, AgentEventKind},
    },
    durability::{SessionSummary, TranscriptItem},
};
use nanocode_observability::TUI_TARGET;
use nanocode_tui::{
    composer::{Composer, ComposerAction},
    crossterm::event::{
        Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    effort::{DialAction, EffortDial},
    history::History,
    picker::{Picker, PickerAction, PickerItem},
    queue::{Queue, QueueKind, QueueState, QueuedPrompt},
    scheduler::RenderScheduler,
    spinner::{LABEL_TICK, Spinner},
    terminal::{SelectionEvent, TerminalSession},
    theme::Theme,
    transcript::{CellKind, NoticeLevel, ToolStatus, Transcript, TranscriptPolicy},
    tree::{AgentTree, NodeStatus},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::{
    events::{Accepted, EventMapper},
    host::{Host, Result},
    models, view,
};

pub(crate) mod reload;

pub const DISPLAY_FRAME: Duration = Duration::from_micros(16_667);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Redraw {
    None,
    Streaming,
    Immediate,
}
pub const SPINNER_FRAME: Duration = Duration::from_millis(80);
pub const QUIT_CONFIRMATION: Duration = Duration::from_secs(2);
pub const AUTOSCROLL_STEP: Duration = Duration::from_millis(50);
pub const TOOL_PREVIEW_LINES: usize = 6;
pub const DIFF_PREVIEW_LINES: usize = 40;

pub enum AppEvent {
    Reload,
    Agent {
        generation: u64,
        event: AgentEvent,
    },
    TurnStarted {
        generation: u64,
        control: TurnControl,
    },
    RoutedTurn {
        generation: u64,
        id: String,
        control: TurnControl,
    },
    Files(std::result::Result<Vec<String>, String>),
    ShellDone {
        index: usize,
        result: std::result::Result<(Option<i32>, String), String>,
        duration: Duration,
    },
    ImagePasted(std::result::Result<PathBuf, String>),
    TurnFinished {
        generation: u64,
        result: std::result::Result<TurnResult, String>,
    },
    PromptFailed {
        generation: u64,
        error: String,
        text: Option<String>,
    },
    PromptBlocked {
        generation: u64,
        error: String,
        durable: bool,
        claimed: bool,
        blocked: PendingTurn,
    },
    OwnerMoved {
        generation: u64,
    },
    Recovered {
        generation: u64,
        result: std::result::Result<Vec<PendingTurn>, String>,
        error: String,
        durable: bool,
        blocked: PendingTurn,
    },
    SteerSent {
        id: String,
        result: std::result::Result<(), String>,
    },
    RunnerListening(std::result::Result<RunnerServer, String>),
    AgentReady(Box<Ready>),
    AgentFailed(String),
    Sessions(std::result::Result<Vec<SessionSummary>, String>),
    Btw {
        id: u64,
        update: BtwUpdate,
    },
    Side {
        pane: u64,
        update: crate::side::SideUpdate,
    },
    Subagent(ScopedAgentUpdate),
    FocusReady(Box<crate::focus::FocusReady>),
    AgentStopped {
        root: String,
        id: nanocode::subagents::AgentId,
        result: std::result::Result<Vec<nanocode::subagents::AgentId>, String>,
    },
    AgentDirectory {
        root: String,
        result: std::result::Result<Vec<nanocode::subagents::AgentDirectoryEntry>, String>,
    },
    AgentsListed {
        root: String,
        result: std::result::Result<String, String>,
    },
    Notice(NoticeLevel, String),
    Applied(std::result::Result<(ModelSelection, Thinking, ServiceTier, String), String>),
}

impl AppEvent {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Reload => "reload",
            Self::Agent { .. } => "agent",
            Self::TurnStarted { .. } => "turn_started",
            Self::RoutedTurn { .. } => "routed_turn",
            Self::Files(_) => "files",
            Self::ShellDone { .. } => "shell_done",
            Self::ImagePasted(_) => "image_pasted",
            Self::TurnFinished { .. } => "turn_finished",
            Self::PromptFailed { .. } => "prompt_failed",
            Self::PromptBlocked { .. } => "prompt_blocked",
            Self::Recovered { .. } => "recovered",
            Self::OwnerMoved { .. } => "owner_moved",
            Self::SteerSent { .. } => "steer_sent",
            Self::RunnerListening(_) => "runner_listening",
            Self::AgentReady(_) => "agent_ready",
            Self::AgentFailed(_) => "agent_failed",
            Self::Sessions(_) => "sessions",
            Self::Btw { .. } => "btw",
            Self::Side { .. } => "side",
            Self::Subagent(_) => "subagent",
            Self::FocusReady(_) => "focus_ready",
            Self::AgentStopped { .. } => "agent_stopped",
            Self::AgentDirectory { .. } => "agent_directory",
            Self::AgentsListed { .. } => "agents_listed",
            Self::Notice(..) => "notice",
            Self::Applied(_) => "applied",
        }
    }
}

pub enum BtwUpdate {
    Message(String),
    Done,
    Failed(String),
}

pub struct Ready {
    pub agent: Agent,
    pub events: AgentEvents,
    pub model: ModelSelection,
    pub thinking: Thinking,
    pub tier: ServiceTier,
    pub history: Option<Vec<TranscriptItem>>,
    pub notice: String,
    pub reset_transcript: bool,
    pub pending: Vec<PendingTurn>,
    pub prompts: Vec<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingTurn {
    pub id: String,
    pub text: String,
    pub prompt: nanocode::agent::input::Prompt,
    #[serde(default)]
    pub shown: bool,
}

pub enum TurnState {
    Idle,
    Starting(Instant),
    External(Instant),
    Running {
        control: TurnControl,
        since: Instant,
    },
}

pub enum PickerPurpose {
    Model,
    Thinking(ModelSelection),
    Tier(ModelSelection, Thinking),
    Session(Vec<SessionSummary>),
    Recent(Vec<String>),
    Theme,
    Skill(Vec<String>),
}

pub enum Overlay {
    Picker(PickerPurpose, Picker),
    Dial {
        model: ModelSelection,
        levels: Vec<Thinking>,
        then_tier: bool,
        dial: EffortDial,
    },
    Help,
}

pub struct Launch {
    pub host: Host,
    pub model: ModelSelection,
    pub thinking: Thinking,
    pub tier: ServiceTier,
    pub resume: Option<String>,
    pub pick_session: bool,
    pub prompt: Option<String>,
    pub history_path: Option<PathBuf>,
    pub home: PathBuf,
    pub thinking_override: Option<Thinking>,
    pub tier_override: Option<ServiceTier>,
    pub reload: Option<reload::State>,
    pub reload_path: Option<PathBuf>,
}

pub struct App {
    pub host: Host,
    pub agent: Option<Agent>,
    pub generation: u64,
    pub model: ModelSelection,
    pub thinking: Thinking,
    pub tier: ServiceTier,
    pub transcript: Transcript,
    pub mapper: EventMapper,
    pub composer: Composer,
    pub queue: Queue,
    pub turn: TurnState,
    pub overlay: Option<Overlay>,
    pub tree: AgentTree,
    pub show_tree: bool,
    pub show_panel: bool,
    pub(crate) panel_refresh: bool,
    pub theme: Theme,
    pub status: Option<(String, Instant)>,
    pub starting: bool,
    pub pending_switch: Option<(ModelSelection, Thinking, ServiceTier)>,
    pub spinner: Spinner,
    toggle_mouse: bool,
    pub(crate) home: PathBuf,
    pub(crate) finder: Option<crate::extras::Finder>,
    pub(crate) finder_dismissed: Option<usize>,
    pub(crate) workspace_files: Option<Vec<String>>,
    pub(crate) files_loading: bool,
    pub(crate) attachments: Vec<PathBuf>,
    attachments_by_id: HashMap<String, Vec<PathBuf>>,
    next_attachments: Vec<PathBuf>,
    resumes: HashMap<String, (nanocode::agent::input::Prompt, bool)>,
    resuming: bool,
    hold_queue: bool,
    moved: bool,
    pub(crate) pending_copy: Option<(String, &'static str)>,
    pub(crate) interrupting: bool,
    thinking_override: Option<Thinking>,
    tier_override: Option<ServiceTier>,
    awaiting_terminal: bool,
    computer_use_notice: Option<String>,
    pub(crate) session_started: bool,
    pub(crate) tx: UnboundedSender<AppEvent>,
    pub(crate) quit: bool,
    reload: bool,
    reload_all: bool,
    pub(crate) submissions: tokio::task::JoinSet<()>,
    reload_side: Option<crate::side::SavedSide>,
    pub(crate) reload_focus: Option<nanocode::subagents::AgentId>,
    quit_armed: Option<Instant>,
    esc_armed: Option<Instant>,
    stashed_draft: Option<String>,
    dragging: Option<bool>,
    autoscroll_at: Option<Instant>,
    pub(crate) clock_deadline: Option<Instant>,
    pub(crate) btw: HashMap<u64, (Option<nanocode::subagents::AgentId>, usize)>,
    pub(crate) next_btw: u64,
    next_local: u64,
    history_path: Option<PathBuf>,
    spans: crate::telemetry::TurnSpans,
    pub(crate) focus: Option<nanocode::subagents::AgentId>,
    pub(crate) views: HashMap<nanocode::subagents::AgentId, crate::focus::SubagentView>,
    pub(crate) panes: crate::side::Panes,
}

pub async fn run(launch: Launch, mut terminal: TerminalSession) -> Result<()> {
    let (tx, mut rx) = unbounded_channel();
    let history = launch
        .history_path
        .as_ref()
        .map(|path| crate::history::load(path))
        .unwrap_or_default();
    let mut app = App {
        host: launch.host.clone(),
        agent: None,
        generation: 0,
        model: launch.model.clone(),
        thinking: launch.thinking,
        tier: launch.tier,
        transcript: Transcript::new(TranscriptPolicy {
            collapsed_preview_lines: TOOL_PREVIEW_LINES,
            collapsed_diff_lines: DIFF_PREVIEW_LINES,
        }),
        mapper: EventMapper::default(),
        composer: Composer::new("Ask anything · / for commands", History::new(history)),
        queue: Queue::default(),
        turn: TurnState::Idle,
        overlay: None,
        tree: AgentTree::default(),
        show_tree: true,
        show_panel: true,
        panel_refresh: false,
        theme: Theme::default(),
        status: None,
        starting: true,
        pending_switch: None,
        spinner: Spinner::new(Instant::now(), SPINNER_FRAME),
        toggle_mouse: false,
        home: launch.home.clone(),
        finder: None,
        finder_dismissed: None,
        workspace_files: None,
        files_loading: false,
        attachments: Vec::new(),
        attachments_by_id: HashMap::new(),
        next_attachments: Vec::new(),
        resumes: HashMap::new(),
        resuming: false,
        hold_queue: false,
        moved: false,
        pending_copy: None,
        interrupting: false,
        thinking_override: launch.thinking_override,
        tier_override: launch.tier_override,
        awaiting_terminal: false,
        computer_use_notice: None,
        session_started: false,
        tx,
        quit: false,
        reload: false,
        reload_all: false,
        submissions: tokio::task::JoinSet::new(),
        reload_side: None,
        reload_focus: None,
        quit_armed: None,
        esc_armed: None,
        stashed_draft: None,
        dragging: None,
        autoscroll_at: None,
        clock_deadline: None,
        btw: HashMap::new(),
        next_btw: 0,
        next_local: 0,
        history_path: launch.history_path.clone(),
        spans: crate::telemetry::TurnSpans::default(),
        focus: None,
        views: HashMap::new(),
        panes: crate::side::Panes::default(),
    };
    let instance = reload::Instance::start(app.tx.clone()).await?;
    if let Some(state) = launch.reload {
        app.restore_reload(state);
    }
    for server in app.host.runtime.runner_servers() {
        app.notice(NoticeLevel::Info, crate::host::listener_notice(&server));
    }
    forward_runners(&app.host.runtime, app.tx.clone());
    if let Some(notices) = app.host.config.clients.notifying() {
        forward_notices(notices, app.tx.clone());
    }
    if let Some(updates) = app.host.runtime.take_subagent_updates() {
        forward_subagents(updates, app.tx.clone());
    }
    if let Some(prompt) = launch
        .prompt
        .clone()
        .filter(|prompt| !prompt.trim().is_empty())
    {
        app.enqueue_follow_up(prompt);
    }
    if launch.pick_session {
        app.starting = false;
        app.open_session_picker();
        if let Some(path) = launch.reload_path {
            tokio::fs::remove_file(path).await?;
        }
    } else {
        let connection = app.connection(launch.resume.clone());
        match connection.await {
            Ok(ready) => {
                app.ready(ready);
                if let Some(path) = launch.reload_path {
                    tokio::fs::remove_file(path).await?;
                }
            }
            Err(error) => {
                app.starting = false;
                app.notice(NoticeLevel::Error, error.to_string());
            }
        }
    }
    let outcome = app.event_loop(&mut terminal, &mut rx).await;
    drop(terminal);
    let session = app.session_id();
    let moved = app.moved;
    let executor = app.reload.then(|| app.reload_executor());
    let hands = app.reload.then(|| app.reload_hands());
    if app.reload_all
        && let Err(error) = instance.broadcast().await
    {
        eprintln!("nanocode: could not reload every window: {error}");
    }
    if let Some(agent) = app.agent.as_ref()
        && let Err(error) = agent.flush().await
        && !moved
    {
        eprintln!("nanocode: could not flush the session: {error}");
    }
    if let Err(error) = app.host.runtime.shutdown().await
        && !moved
    {
        eprintln!("nanocode: shutdown did not complete cleanly: {error}");
    }
    while let Some(result) = app.submissions.join_next().await {
        if let Err(error) = result {
            eprintln!("nanocode: prompt task failed during shutdown: {error}");
        }
    }
    if app.reload && outcome.is_ok() {
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::Agent { generation, event } if generation == app.generation => {
                    app.agent_event(&event);
                }
                AppEvent::SteerSent { .. } => {
                    app.handle_app(event);
                }
                AppEvent::Side {
                    pane,
                    update:
                        update @ (crate::side::SideUpdate::Event(_)
                        | crate::side::SideUpdate::Forked(_)
                        | crate::side::SideUpdate::Restored { .. }),
                } => {
                    app.side_update(pane, update);
                }
                _ => {}
            }
        }
        let mut state = app.reload_state();
        if let Some(executor) = executor {
            state.executor = executor;
        }
        if let Some(hands) = hands {
            state.hands = hands;
        }
        let state = instance.save(&state).await?;
        instance.close().await?;
        return Err(reload(session, state).into());
    }
    instance.close().await?;
    if let Some(agent_session) = session {
        eprintln!("{} resume {agent_session}", invoked_name());
    }
    outcome
}

fn invoked_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|arg| {
            std::path::Path::new(&arg)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "nanocode".to_owned())
}

fn reload_program() -> std::io::Result<PathBuf> {
    let name = invoked_name();
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(&name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    std::env::current_exe()
}

fn reload_args(session: Option<&str>, state: PathBuf) -> Vec<std::ffi::OsString> {
    let mut command = <crate::cli::Cli as clap::CommandFactory>::command();
    command.build();
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    let mut original = std::env::args_os().skip(1).peekable();
    while let Some(arg) = original.next() {
        let text = arg.to_string_lossy().into_owned();
        if text == "--" {
            break;
        }
        let (definition, inline) = if let Some(long) = text.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value)));
            (
                command
                    .get_arguments()
                    .find(|arg| arg.get_long() == Some(name)),
                value.is_some(),
            )
        } else if let Some(short) = text.strip_prefix('-').filter(|short| !short.is_empty()) {
            let mut chars = short.chars();
            let flag = chars.next();
            (
                command.get_arguments().find(|arg| arg.get_short() == flag),
                !chars.as_str().is_empty(),
            )
        } else {
            continue;
        };
        let Some(definition) = definition else {
            args.push(arg);
            continue;
        };
        let value = (!inline && definition.get_action().takes_values())
            .then(|| {
                let required = definition
                    .get_num_args()
                    .is_some_and(|range| range.min_values() > 0);
                let present = original
                    .peek()
                    .is_some_and(|next| !next.to_string_lossy().starts_with('-'));
                (required || present).then(|| original.next()).flatten()
            })
            .flatten();
        if matches!(
            definition.get_id().as_str(),
            "resume" | "continue_last" | "reload_state"
        ) {
            continue;
        }
        args.push(arg);
        args.extend(value);
    }
    if let Some(session) = session {
        args.push("--resume".into());
        args.push(session.into());
    }
    args.push("--reload-state".into());
    args.push(state.into_os_string());
    args
}

#[cfg(unix)]
fn reload(session: Option<String>, state: PathBuf) -> String {
    use std::os::unix::process::CommandExt;
    let program = match reload_program() {
        Ok(program) => program,
        Err(error) => return format!("reload failed to locate nanocode: {error}"),
    };
    let error = std::process::Command::new(&program)
        .args(reload_args(session.as_deref(), state.clone()))
        .exec();
    format!(
        "reload failed to start {}: {error}; saved window state: {}",
        program.display(),
        state.display()
    )
}

#[cfg(not(unix))]
fn reload(session: Option<String>, _state: PathBuf) -> String {
    match session {
        Some(session) => format!(
            "reload is not supported on this platform; run {} resume {session}",
            invoked_name()
        ),
        None => "reload is not supported on this platform".to_owned(),
    }
}

fn runner_status_label(status: &RunnerStatus) -> String {
    match status {
        RunnerStatus::Connecting => "connecting".to_owned(),
        RunnerStatus::Ready => "connected".to_owned(),
        RunnerStatus::Disconnected => "disconnected".to_owned(),
        RunnerStatus::Failed(error) => format!("failed: {error}"),
    }
}

fn runner_info(info: &RunnerInfo) -> String {
    let workspace = info
        .workspace
        .as_ref()
        .map(|workspace| workspace.display().to_string())
        .unwrap_or_else(|| "pending workspace".into());
    let kind = match info.protocol {
        RunnerProtocol::Tools => "tool runner",
        RunnerProtocol::CodeMode => "hand",
    };
    format!(
        "{} {} ({kind}, {}, {:?}, {} tools, {workspace})",
        info.name,
        runner_status_label(&info.status),
        info.endpoint,
        info.mode,
        info.tool_count
    )
}

fn forward_runners(runtime: &nanocode::Nanocode, tx: UnboundedSender<AppEvent>) {
    let mut events = runtime.runner_events();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let (level, message) = match event {
                RunnerEvent::StatusChanged { info } => (NoticeLevel::Info, runner_info(&info)),
                RunnerEvent::Failed { name, error } => {
                    (NoticeLevel::Error, format!("runner {name}: {error}"))
                }
            };
            if tx.send(AppEvent::Notice(level, message)).is_err() {
                break;
            }
        }
    });
}

fn forward_notices(mut notices: crate::host::Notices, tx: UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        while let Some(notice) = notices.next().await {
            if tx
                .send(AppEvent::Notice(NoticeLevel::Info, notice))
                .is_err()
            {
                break;
            }
        }
    });
}

fn forward_subagents(mut updates: nanocode::SubagentUpdates, tx: UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        while let Some(update) = updates.recv().await {
            if tx.send(AppEvent::Subagent(update)).is_err() {
                break;
            }
        }
    });
}

impl App {
    pub fn session_id(&self) -> Option<String> {
        self.agent
            .as_ref()
            .map(|agent| agent.session_id().to_owned())
    }

    async fn event_loop(
        &mut self,
        terminal: &mut TerminalSession,
        rx: &mut UnboundedReceiver<AppEvent>,
    ) -> Result<()> {
        let mut input = EventStream::new();
        let mut scheduler = RenderScheduler::new(DISPLAY_FRAME, Instant::now());
        let mut metrics = crate::telemetry::FrameMetrics::new();
        let mut dirty = true;
        let mut full_at: Option<Instant> = None;
        while !self.quit {
            while let Some(result) = self.submissions.try_join_next() {
                if let Err(error) = result {
                    self.notice(NoticeLevel::Error, format!("prompt task failed: {error}"));
                }
            }
            metrics.wakeup();
            let now = Instant::now();
            if self.autoscroll_at.is_some_and(|at| at <= now) {
                self.autoscroll_at = None;
                if self.autoscroll() {
                    self.autoscroll_at = Some(now + AUTOSCROLL_STEP);
                    dirty = true;
                    scheduler.request_immediate(now);
                }
            }
            if scheduler.is_due(now) {
                let span = metrics.frame(self.agent.as_ref().map(|agent| agent.session_id()));
                let _frame = span.enter();
                let frame_started = metrics.begin();
                let cpu_started = crate::telemetry::frame_cpu(&span);
                let animated = !dirty
                    && self.animating()
                    && full_at.is_some_and(|at| now < at)
                    && match terminal.animate(self.spinner.frame(now)) {
                        Ok(animated) => animated,
                        Err(error) => {
                            span.record("otel.status_code", "ERROR");
                            return Err(error.into());
                        }
                    };
                if !animated {
                    if let Err(error) = terminal.draw(|frame| view::draw(self, frame)) {
                        span.record("otel.status_code", "ERROR");
                        return Err(error.into());
                    }
                    dirty = false;
                    let at = self
                        .clock_deadline
                        .map_or(now + LABEL_TICK, |at| at.min(now + LABEL_TICK));
                    full_at = Some(match &self.status {
                        Some((_, shown)) => at.min(*shown + view::STATUS_VISIBLE),
                        None => at,
                    });
                }
                crate::telemetry::record_frame_cpu(&span, cpu_started);
                metrics.presented(frame_started, &span);
                scheduler.presented(Instant::now());
                if self.animating() {
                    scheduler.request_at(self.spinner.next_deadline(Instant::now()));
                }
                if (self.animating() || self.panel_refresh)
                    && let Some(at) = full_at
                {
                    scheduler.request_at(at);
                }
                if let Some((_, at)) = &self.status {
                    let expiry = *at + view::STATUS_VISIBLE;
                    if expiry > Instant::now() {
                        scheduler.request_at(expiry);
                    }
                }
            }
            if let Some(at) = self.autoscroll_at {
                scheduler.request_at(at);
            }
            let deadline = scheduler.deadline().map(tokio::time::Instant::from_std);
            tokio::select! {
                biased;
                event = input.next() => {
                    match event {
                        Some(Ok(event)) => {
                            let mut pending = Some(event);
                            while let Some(event) = pending.take() {
                                let resize = matches!(event, Event::Resize(..));
                                let span = crate::telemetry::input_span(&event);
                                span.record("session.id", self.agent.as_ref().map(|agent| agent.session_id()));
                                let changed = span.in_scope(|| self.handle_terminal(event, terminal));
                                span.record("input.redraw", changed);
                                if changed {
                                    metrics.input(resize, &span);
                                    scheduler.request_immediate(Instant::now());
                                }
                                dirty |= changed;
                                match std::future::poll_fn(|context| {
                                    Poll::Ready(input.poll_next_unpin(context))
                                })
                                .await
                                {
                                    Poll::Ready(Some(Ok(event))) if !self.quit => {
                                        pending = Some(event);
                                    }
                                    Poll::Ready(Some(Err(error))) => return Err(error.into()),
                                    _ => {}
                                }
                            }
                        }
                        Some(Err(error)) => return Err(error.into()),
                        None => break,
                    }
                }
                event = rx.recv() => {
                    let Some(event) = event else { break };
                    let until = scheduler.next_frame_at(Instant::now());
                    let mut redraw = self.handle_app(event);
                    while !self.quit
                        && Instant::now() < until
                        && let Ok(event) = rx.try_recv()
                    {
                        redraw = redraw.max(self.handle_app(event));
                    }
                    match redraw {
                        Redraw::Immediate => {
                            metrics.stream();
                            dirty = true;
                            scheduler.request_immediate(Instant::now());
                        }
                        Redraw::Streaming => {
                            metrics.stream();
                            dirty = true;
                            scheduler.request_streaming(Instant::now());
                        }
                        Redraw::None => {}
                    }
                }
                () = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
        }
        Ok(())
    }

    fn animating(&self) -> bool {
        !matches!(self.turn, TurnState::Idle)
            || self.starting
            || !self.btw.is_empty()
            || self.panes.busy()
            || self.tree.active() > 0
    }

    pub fn notice(&mut self, level: NoticeLevel, text: impl Into<String>) {
        self.transcript.push(CellKind::Notice(level, text.into()));
    }

    pub fn flash(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    pub(crate) fn local_id_pub(&mut self, prefix: &str) -> String {
        self.local_id(prefix)
    }

    fn local_id(&mut self, prefix: &str) -> String {
        self.next_local += 1;
        format!("{prefix}-{}-{}", std::process::id(), self.next_local)
    }

    pub fn connect(&mut self, resume: Option<String>) {
        let ready = self.connection(resume);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let event = match ready.await {
                Ok(ready) => AppEvent::AgentReady(Box::new(ready)),
                Err(error) => AppEvent::AgentFailed(error.to_string()),
            };
            drop(tx.send(event));
        });
    }

    fn connection(
        &mut self,
        resume: Option<String>,
    ) -> impl Future<Output = Result<Ready>> + Send + 'static {
        self.starting = true;
        let thinking_override = self.thinking_override.take();
        let tier_override = self.tier_override.take();
        let host = self.host.clone();
        let model = self.model.clone();
        let thinking = self.thinking;
        let tier = self.tier;
        let span = tracing::info_span!(target: "nanocode", "nc.session.connect", model = %model, resume = resume.is_some(), otel.status_code = tracing::field::Empty);
        tracing::Instrument::instrument(
            async move {
                let ready = async {
                    match resume {
                        Some(session_id) => {
                            open_session(&host, &session_id, thinking_override, tier_override).await
                        }
                        None => {
                            let (agent, events) = host.start(&model, thinking).await?;
                            apply_tier(&agent, &model, tier).await?;
                            Ok(Ready {
                                notice: format!("new session {}", agent.session_id()),
                                agent,
                                events,
                                model,
                                thinking,
                                tier,
                                history: None,
                                reset_transcript: false,
                                pending: Vec::new(),
                                prompts: Vec::new(),
                            })
                        }
                    }
                };
                ready.await.inspect_err(|_| {
                    tracing::Span::current().record("otel.status_code", "ERROR");
                })
            },
            span,
        )
    }

    fn handle_app(&mut self, event: AppEvent) -> Redraw {
        let _span = tracing::info_span!(target: TUI_TARGET, "nc.app_event", event.kind = event.kind(), session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        match event {
            AppEvent::Reload => {
                self.request_reload(false);
                Redraw::Immediate
            }
            AppEvent::Agent { generation, event } => {
                if generation == self.generation {
                    self.agent_event(&event)
                } else {
                    Redraw::None
                }
            }
            AppEvent::TurnStarted {
                generation,
                control,
            } => {
                if generation != self.generation {
                    return Redraw::None;
                }
                let since = match self.turn {
                    TurnState::Starting(since) => since,
                    _ => Instant::now(),
                };
                self.turn = TurnState::Running { control, since };
                self.flush_pending_steers();
                Redraw::Immediate
            }
            AppEvent::Files(files) => {
                self.files_loaded(files);
                Redraw::Immediate
            }
            AppEvent::ShellDone {
                index,
                result,
                duration,
            } => {
                self.shell_done(index, result, duration);
                Redraw::Immediate
            }
            AppEvent::ImagePasted(result) => {
                match result {
                    Ok(path) => self.attach(path),
                    Err(error) => self.flash(format!("image paste failed: {error}")),
                }
                Redraw::Immediate
            }
            AppEvent::RoutedTurn {
                generation,
                id,
                control,
            } => {
                if generation != self.generation {
                    return Redraw::None;
                }
                self.queue.convert(&id, QueueKind::FollowUp);
                self.queue.set_state(&id, QueueState::Sent);
                self.resuming = false;
                self.mapper.begin_turn();
                self.turn = TurnState::Running {
                    control,
                    since: Instant::now(),
                };
                self.flush_pending_steers();
                Redraw::Immediate
            }
            AppEvent::TurnFinished { generation, result } => {
                if generation != self.generation {
                    return Redraw::None;
                }
                self.turn = TurnState::Idle;
                if std::mem::take(&mut self.resuming) && result.is_err() && !self.interrupting {
                    self.hold_after_failed_resume();
                }
                match result {
                    Err(error) if !self.mapper.reported() => {
                        self.transcript.finish_running_tools(ToolStatus::Failed);
                        if !self.transcript.has_notice_since(
                            self.mapper.turn_start(),
                            NoticeLevel::Error,
                            &error,
                        ) {
                            self.notice(NoticeLevel::Error, error);
                        }
                        self.settle_turn();
                    }
                    _ if self.mapper.reported() => self.settle_turn(),
                    _ => self.awaiting_terminal = true,
                }
                Redraw::Immediate
            }
            AppEvent::PromptFailed {
                generation,
                error,
                text,
            } => {
                if generation != self.generation {
                    return Redraw::None;
                }
                self.turn = TurnState::Idle;
                self.resuming = false;
                let steers = self.queue.drain_kind(QueueKind::Steer);
                self.remove_sent_prompt();
                self.restore_prompt(&error, text, steers);
                Redraw::Immediate
            }
            AppEvent::PromptBlocked {
                generation,
                error,
                durable,
                claimed,
                blocked,
            } => {
                if generation != self.generation {
                    return Redraw::None;
                }
                self.turn = TurnState::Idle;
                self.resuming = false;
                self.remove_sent_prompt();
                if claimed {
                    self.after_turn();
                } else {
                    self.recover(error, durable, blocked);
                }
                Redraw::Immediate
            }
            AppEvent::OwnerMoved { generation } => {
                if generation != self.generation || self.moved {
                    return Redraw::None;
                }
                self.moved = true;
                self.hold_queue = true;
                self.notice(NoticeLevel::Warning, OWNER_MOVED);
                Redraw::Immediate
            }
            AppEvent::Recovered {
                generation,
                result,
                error,
                durable,
                blocked,
            } => {
                if generation != self.generation {
                    return Redraw::None;
                }
                match result {
                    Ok(pending) if pending.first().is_some_and(|turn| turn.id != blocked.id) => {
                        if matches!(self.turn, TurnState::Idle) {
                            self.resume_pending(pending, Some((blocked, durable)));
                        } else {
                            self.hold(blocked, durable);
                        }
                    }
                    Ok(_) => {
                        self.restore_prompt(&error, (!durable).then_some(blocked.text), Vec::new());
                    }
                    Err(load) => {
                        self.notice(
                            NoticeLevel::Error,
                            format!("could not load interrupted turns: {load}"),
                        );
                        self.restore_prompt(&error, (!durable).then_some(blocked.text), Vec::new());
                    }
                }
                Redraw::Immediate
            }
            AppEvent::SteerSent { id, result } => {
                match result {
                    Ok(()) => {
                        self.queue.set_state(&id, QueueState::Sent);
                    }
                    Err(error) => {
                        if self.queue.convert(&id, QueueKind::FollowUp) {
                            self.flash(format!(
                                "steer not admitted ({error}); queued as follow-up"
                            ));
                        }
                        if matches!(self.turn, TurnState::Idle) {
                            self.after_turn();
                        }
                    }
                }
                Redraw::Immediate
            }
            AppEvent::AgentReady(ready) => {
                self.ready(*ready);
                Redraw::Immediate
            }
            AppEvent::AgentFailed(error) => {
                self.starting = false;
                self.notice(
                    NoticeLevel::Error,
                    format!("failed to start agent: {error}"),
                );
                Redraw::Immediate
            }
            AppEvent::Sessions(result) => {
                match result {
                    Ok(sessions) if sessions.is_empty() => {
                        self.flash("no saved sessions for this workspace");
                        if self.agent.is_none() && !self.starting {
                            self.connect(None);
                        }
                    }
                    Ok(sessions) => {
                        let picker = crate::sessions::picker(&sessions);
                        self.overlay =
                            Some(Overlay::Picker(PickerPurpose::Session(sessions), picker));
                    }
                    Err(error) => {
                        self.notice(
                            NoticeLevel::Error,
                            format!("failed to list sessions: {error}"),
                        );
                        if self.agent.is_none() && !self.starting {
                            self.connect(None);
                        }
                    }
                }
                Redraw::Immediate
            }
            AppEvent::Btw { id, update } => {
                self.btw_update(id, update);
                Redraw::Streaming
            }
            AppEvent::Side { pane, update } => self.side_update(pane, update),
            AppEvent::Subagent(update) => self.subagent(update),
            AppEvent::RunnerListening(result) => {
                match result {
                    Ok(server) => {
                        self.notice(NoticeLevel::Info, crate::host::listener_notice(&server));
                    }
                    Err(error) => self.notice(NoticeLevel::Error, error),
                }
                Redraw::Immediate
            }
            AppEvent::Notice(level, text) => {
                self.notice(level, text);
                Redraw::Immediate
            }
            AppEvent::FocusReady(ready) => {
                self.focus_ready(*ready);
                Redraw::Immediate
            }
            AppEvent::AgentStopped { root, id, result } => {
                if self.session_id().as_deref() == Some(root.as_str()) {
                    self.agent_stopped(id, result);
                }
                Redraw::Immediate
            }
            AppEvent::AgentDirectory { root, result } => {
                if self.session_id().as_deref() == Some(root.as_str()) {
                    self.agent_directory(result);
                }
                Redraw::Immediate
            }
            AppEvent::AgentsListed { root, result } => {
                if self.session_id().as_deref() == Some(root.as_str()) {
                    match result {
                        Ok(text) => self.notice(NoticeLevel::Info, text),
                        Err(error) => {
                            self.notice(NoticeLevel::Error, format!("listing subagents: {error}"))
                        }
                    }
                }
                Redraw::Immediate
            }
            AppEvent::Applied(result) => {
                self.starting = false;
                match result {
                    Ok((model, thinking, tier, notice)) => {
                        self.model = model;
                        self.thinking = thinking;
                        self.tier = tier;
                        self.notice(NoticeLevel::Info, notice);
                    }
                    Err(error) => {
                        self.notice(NoticeLevel::Error, format!("failed to switch: {error}"))
                    }
                }
                self.after_turn();
                Redraw::Immediate
            }
        }
    }

    fn ready(&mut self, ready: Ready) {
        self.starting = false;
        self.moved = false;
        self.generation += 1;
        if !ready.reset_transcript
            && self
                .agent
                .as_ref()
                .is_some_and(|agent| agent.session_id() != ready.agent.session_id())
        {
            self.tree = AgentTree::default();
            self.reset_focus();
        }
        if ready.reset_transcript {
            self.transcript.clear();
            self.mapper.reset();
            self.tree = AgentTree::default();
            self.reset_focus();
            if self.panes.side.is_some() {
                self.close_side();
            }
        }
        self.session_started = ready
            .history
            .as_ref()
            .is_some_and(|history| !history.is_empty())
            || !ready.prompts.is_empty()
            || !ready.pending.is_empty();
        match ready.history {
            Some(history) if !history.is_empty() => {
                crate::sessions::replay(&mut self.transcript, history);
            }
            _ => {
                for prompt in &ready.prompts {
                    self.transcript.push(CellKind::User(prompt.clone()));
                }
            }
        }
        self.model = ready.model;
        self.thinking = ready.thinking;
        self.tier = ready.tier;
        self.turn = TurnState::Idle;
        self.awaiting_terminal = false;
        let generation = self.generation;
        let tx = self.tx.clone();
        let mut events = ready.events;
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if tx.send(AppEvent::Agent { generation, event }).is_err() {
                    break;
                }
            }
        });
        self.agent = Some(ready.agent);
        if let Some(side) = self.reload_side.take() {
            self.restore_side(side);
        }
        self.refresh_agents();
        self.notice(NoticeLevel::Info, ready.notice);
        let unavailable = self.host.runtime.computer_use_unavailable();
        if unavailable.is_some() && unavailable != self.computer_use_notice {
            self.computer_use_notice.clone_from(&unavailable);
            if let Some(reason) = unavailable {
                self.notice(
                    NoticeLevel::Warning,
                    format!("computer use unavailable, continuing without it: {reason}"),
                );
            }
        }
        if !ready.pending.is_empty() {
            self.resume_pending(ready.pending, None);
            return;
        }
        self.after_turn();
    }

    fn resume_pending(&mut self, pending: Vec<PendingTurn>, blocked: Option<(PendingTurn, bool)>) {
        let Some(agent) = self.agent.clone() else {
            return;
        };
        let resumed = pending
            .iter()
            .map(|turn| turn.id.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut pending = pending.into_iter();
        let Some(first) = pending.next() else {
            return;
        };
        let queued = self
            .queue
            .drain_kind(QueueKind::FollowUp)
            .into_iter()
            .filter(|item| !resumed.contains(&item.id))
            .collect::<Vec<_>>();
        for turn in pending {
            self.hold(turn, true);
        }
        if let Some((blocked, durable)) =
            blocked.filter(|(blocked, _)| !resumed.contains(&blocked.id))
        {
            self.hold(blocked, durable);
        }
        for item in queued {
            self.queue.push(item);
        }
        self.resumes.remove(&first.id);
        if first.shown {
            self.mapper.replayed_prompt(first.id.clone());
        }
        self.notice(NoticeLevel::Info, "continuing the interrupted turn…");
        self.launch_turn(agent, first.id, first.text, first.prompt, true);
    }

    fn hold(&mut self, turn: PendingTurn, durable: bool) {
        self.queue.push(QueuedPrompt {
            id: turn.id.clone(),
            kind: QueueKind::FollowUp,
            state: QueueState::Pending,
            text: turn.text,
        });
        self.resumes.insert(turn.id, (turn.prompt, durable));
    }

    fn recover(&mut self, error: String, durable: bool, blocked: PendingTurn) {
        let (Some(store), Some(agent)) = (self.host.config.store.clone(), self.agent.as_ref())
        else {
            self.restore_prompt(&error, (!durable).then_some(blocked.text), Vec::new());
            return;
        };
        let session_id = agent.session_id().to_owned();
        let generation = self.generation;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = match store.turns(&session_id).await {
                Ok(turns) => pending_turns(&turns, None).map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            drop(tx.send(AppEvent::Recovered {
                generation,
                result,
                error,
                durable,
                blocked,
            }));
        });
    }

    fn hold_after_failed_resume(&mut self) {
        self.hold_queue = true;
        self.flash("the resumed turn did not finish · queued prompts wait for your next message");
    }

    fn remove_sent_prompt(&mut self) {
        if let Some(item) = self
            .queue
            .items()
            .iter()
            .find(|item| item.state == QueueState::Sent)
            .map(|item| item.id.clone())
        {
            self.queue.remove(&item);
        }
    }

    fn restore_prompt(&mut self, error: &str, text: Option<String>, steers: Vec<QueuedPrompt>) {
        self.notice(NoticeLevel::Error, format!("prompt failed: {error}"));
        for steer in &steers {
            if let Some(attachments) = self.attachments_by_id.remove(&steer.id) {
                self.attachments.extend(attachments);
            }
        }
        if text.is_none() {
            self.hold_after_failed_resume();
        }
        let mut restored = text
            .into_iter()
            .chain(steers.into_iter().map(|steer| steer.text))
            .collect::<Vec<_>>();
        if restored.is_empty() {
            return;
        }
        if !self.composer.is_empty() {
            restored.push(self.composer.text().to_owned());
        }
        self.composer.set_text(restored.join("\n\n"));
    }

    fn agent_event(&mut self, event: &AgentEvent) -> Redraw {
        let session_id = self.agent.as_ref().map(Agent::session_id);
        self.spans.event(event, session_id);
        if event.kind == AgentEventKind::ToolCall {
            self.mapper
                .set_runners(crate::events::runner_routes(&self.host.runtime));
        }
        let mapped = self.mapper.apply(&mut self.transcript, event);
        let shown = self.focus.and_then(|id| self.views.get(&id)).is_none();
        let redraw = if mapped.changed && shown
            || mapped.accepted.is_some()
            || event.kind == AgentEventKind::RunStarted
            || event.kind.is_terminal()
        {
            Redraw::Streaming
        } else {
            Redraw::None
        };
        if event.kind == AgentEventKind::RunStarted
            && matches!(self.turn, TurnState::Idle)
            && !self.starting
        {
            self.turn = TurnState::External(Instant::now());
        }
        if event.kind.is_terminal() && matches!(self.turn, TurnState::External(_)) {
            self.turn = TurnState::Idle;
            self.settle_turn();
        }
        if mapped.accepted.is_some() {
            self.session_started = true;
        }
        if event.kind.is_terminal()
            && self.awaiting_terminal
            && matches!(self.turn, TurnState::Idle)
        {
            self.settle_turn();
        }
        match mapped.accepted {
            Some(Accepted::Steer { id, text }) => {
                let removed = id.and_then(|id| self.queue.remove(&id));
                if removed.is_none() {
                    let found = self
                        .queue
                        .items()
                        .iter()
                        .find(|item| item.kind == QueueKind::Steer && item.text == text)
                        .map(|item| item.id.clone());
                    if let Some(found) = found {
                        self.queue.remove(&found);
                        self.resumes.remove(&found);
                        self.attachments_by_id.remove(&found);
                    }
                }
                if let Some(item) = removed {
                    self.resumes.remove(&item.id);
                    self.attachments_by_id.remove(&item.id);
                }
            }
            Some(Accepted::Prompt { request_id }) => {
                let found = request_id
                    .and_then(|request| self.queue.remove(&request))
                    .or_else(|| {
                        let sent = self
                            .queue
                            .items()
                            .iter()
                            .find(|item| {
                                item.kind == QueueKind::FollowUp && item.state == QueueState::Sent
                            })
                            .map(|item| item.id.clone())?;
                        self.queue.remove(&sent)
                    });
                if let Some(item) = found {
                    self.resumes.remove(&item.id);
                    self.attachments_by_id.remove(&item.id);
                }
            }
            None => {}
        }
        redraw
    }

    fn settle_turn(&mut self) {
        if self.quit {
            return;
        }
        self.awaiting_terminal = false;
        let unaccepted = self
            .queue
            .items()
            .iter()
            .filter(|item| item.kind == QueueKind::Steer && item.state == QueueState::Sent)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        for id in unaccepted {
            self.queue.convert(&id, QueueKind::FollowUp);
        }
        self.after_turn();
    }

    fn after_turn(&mut self) {
        if self.quit || !matches!(self.turn, TurnState::Idle) || self.agent.is_none() {
            return;
        }
        if self.interrupting || self.esc_armed.is_some() {
            self.status = None;
            self.esc_armed = None;
        }
        self.interrupting = false;
        if let Some((model, thinking, tier)) = self.pending_switch.take() {
            self.switch(model, thinking, tier);
            return;
        }
        if self
            .queue
            .items()
            .iter()
            .any(|item| item.state == QueueState::Sent && item.kind == QueueKind::FollowUp)
        {
            return;
        }
        if self.hold_queue {
            return;
        }
        let next = self
            .queue
            .items()
            .iter()
            .find(|item| item.kind == QueueKind::FollowUp || item.state == QueueState::Pending)
            .map(|item| item.id.clone());
        let Some(id) = next else { return };
        let Some(item) = self.queue.remove(&id) else {
            return;
        };
        if let Some((prompt, durable)) = self.resumes.remove(&id) {
            if let Some(agent) = self.agent.clone() {
                self.launch_turn(agent, id, item.text, prompt, durable);
            }
            return;
        }
        let mut text = item.text;
        let mut attachments = self.attachments_by_id.remove(&id).unwrap_or_default();
        let extra = self.queue.drain_kind(QueueKind::Steer);
        for steer in extra {
            text.push_str("\n\n");
            text.push_str(&steer.text);
            attachments.extend(self.attachments_by_id.remove(&steer.id).unwrap_or_default());
        }
        self.next_attachments = attachments;
        self.start_turn(text);
    }

    pub(crate) fn start_turn(&mut self, text: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "start_turn", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let attachments = std::mem::take(&mut self.next_attachments);
        let Some(agent) = self.agent.clone() else {
            let id = self.enqueue_follow_up(text);
            self.attachments_by_id.insert(id, attachments);
            return;
        };
        let id = uuid::Uuid::now_v7().to_string();
        let sent = crate::extras::prompt_for(&text, &attachments);
        self.launch_turn(agent, id, text, sent, false);
    }

    fn launch_turn(
        &mut self,
        agent: Agent,
        id: String,
        text: String,
        sent: nanocode::agent::input::Prompt,
        durable: bool,
    ) {
        self.resuming = durable;
        self.resumes.insert(id.clone(), (sent.clone(), durable));
        self.queue.push(QueuedPrompt {
            id: id.clone(),
            kind: QueueKind::FollowUp,
            state: QueueState::Sent,
            text: text.clone(),
        });
        self.turn = TurnState::Starting(Instant::now());
        self.mapper.begin_turn();
        self.mapper.mark_turn_start(self.transcript.len());
        self.transcript.scroll_to_bottom();
        let generation = self.generation;
        let tx = self.tx.clone();
        let span = self.spans.begin(&id, Some(agent.session_id()));
        self.submissions.spawn(tracing::Instrument::instrument(
            async move {
                let request = PromptRequest::new(sent.clone()).request_id(id.clone());
                match agent.prompt(request).await {
                    Ok(turn) => {
                        drop(tx.send(AppEvent::TurnStarted {
                            generation,
                            control: turn.control(),
                        }));
                        let result = turn.result().await.map_err(|error| {
                            report_owner_moved(&tx, generation, &error);
                            error.to_string()
                        });
                        drop(tx.send(AppEvent::TurnFinished { generation, result }));
                    }
                    Err(error)
                        if error.execution_policy_disposition()
                            == Some(ExecutionPolicyDisposition::Retry) =>
                    {
                        drop(tx.send(AppEvent::PromptBlocked {
                            generation,
                            error: error.to_string(),
                            durable,
                            claimed: durable && operation_active(&error),
                            blocked: PendingTurn {
                                id,
                                text,
                                prompt: sent,
                                shown: false,
                            },
                        }));
                    }
                    Err(error) => {
                        report_owner_moved(&tx, generation, &error);
                        drop(tx.send(AppEvent::PromptFailed {
                            generation,
                            error: error.to_string(),
                            text: (!durable).then_some(text),
                        }));
                    }
                }
            },
            span,
        ));
    }

    fn enqueue_follow_up(&mut self, text: String) -> String {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "enqueue_follow_up", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let id = self.local_id("follow");
        self.queue.push(QueuedPrompt {
            id: id.clone(),
            kind: QueueKind::FollowUp,
            state: QueueState::Pending,
            text,
        });
        id
    }

    fn enqueue_steer(&mut self, text: String) -> String {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "enqueue_steer", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let id = self.local_id("steer");
        self.queue.push(QueuedPrompt {
            id: id.clone(),
            kind: QueueKind::Steer,
            state: QueueState::Pending,
            text,
        });
        id
    }

    fn flush_pending_steers(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "flush_pending_steers", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if self.quit {
            return;
        }
        if matches!(self.turn, TurnState::External(_)) {
            self.route_pending_steers();
            return;
        }
        let TurnState::Running { control, .. } = &self.turn else {
            return;
        };
        let pending = self
            .queue
            .items()
            .iter()
            .filter(|item| item.kind == QueueKind::Steer && item.state == QueueState::Pending)
            .map(|item| (item.id.clone(), item.text.clone()))
            .collect::<Vec<_>>();
        let control = control.clone();
        for (id, text) in pending {
            self.queue.set_state(&id, QueueState::Sent);
            let prompt = self
                .resumes
                .get(&id)
                .map(|(prompt, _)| prompt.clone())
                .unwrap_or_else(|| {
                    crate::extras::prompt_for(
                        &text,
                        self.attachments_by_id.get(&id).map_or(&[], Vec::as_slice),
                    )
                });
            let control = control.clone();
            let tx = self.tx.clone();
            let generation = self.generation;
            self.submissions.spawn(async move {
                let result = control
                    .steer_with_id(id.clone(), prompt)
                    .await
                    .map_err(|error| {
                        report_owner_moved(&tx, generation, &error);
                        error.to_string()
                    });
                drop(tx.send(AppEvent::SteerSent { id, result }));
            });
        }
    }

    fn route_pending_steers(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "route_pending_steers", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(agent) = self.agent.clone() else {
            return;
        };
        let pending = self
            .queue
            .items()
            .iter()
            .filter(|item| item.kind == QueueKind::Steer && item.state == QueueState::Pending)
            .map(|item| (item.id.clone(), item.text.clone()))
            .collect::<Vec<_>>();
        let generation = self.generation;
        for (id, text) in pending {
            self.queue.set_state(&id, QueueState::Sent);
            let prompt = self
                .resumes
                .get(&id)
                .map(|(prompt, _)| prompt.clone())
                .unwrap_or_else(|| {
                    crate::extras::prompt_for(
                        &text,
                        self.attachments_by_id.get(&id).map_or(&[], Vec::as_slice),
                    )
                });
            let agent = agent.clone();
            let tx = self.tx.clone();
            self.submissions.spawn(async move {
                match agent.route_prompt(prompt).await {
                    Ok(PromptRoute::Steered) => {
                        drop(tx.send(AppEvent::SteerSent { id, result: Ok(()) }))
                    }
                    Ok(PromptRoute::Started(turn)) => {
                        drop(tx.send(AppEvent::RoutedTurn {
                            generation,
                            id,
                            control: turn.control(),
                        }));
                        let result = turn.result().await.map_err(|error| {
                            report_owner_moved(&tx, generation, &error);
                            error.to_string()
                        });
                        drop(tx.send(AppEvent::TurnFinished { generation, result }));
                    }
                    Err(error) => {
                        report_owner_moved(&tx, generation, &error);
                        drop(tx.send(AppEvent::SteerSent {
                            id,
                            result: Err(error.to_string()),
                        }));
                    }
                }
            });
        }
    }

    fn interrupt(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "interrupt", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if matches!(self.turn, TurnState::External(_)) {
            let Some(agent) = self.agent.clone() else {
                return;
            };
            self.interrupting = true;
            let tx = self.tx.clone();
            tokio::spawn(async move {
                match agent.cancel_active_turn().await {
                    Ok(()) | Err(nanocode::NanocodeError::TurnNotCancellable) => {}
                    Err(error) => drop(tx.send(AppEvent::Notice(
                        NoticeLevel::Error,
                        format!("interrupt failed: {error}"),
                    ))),
                }
            });
            return;
        }
        let TurnState::Running { control, .. } = &self.turn else {
            if matches!(self.turn, TurnState::Starting(_)) {
                self.flash("turn is starting; press esc again once it runs");
            }
            return;
        };
        let control = control.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Err(error) = control.cancel().await {
                drop(tx.send(AppEvent::Notice(
                    NoticeLevel::Error,
                    format!("interrupt failed: {error}"),
                )));
            }
        });
        let restored = self
            .queue
            .items()
            .iter()
            .filter(|item| !(item.kind == QueueKind::FollowUp && item.state == QueueState::Sent))
            .map(|item| item.text.clone())
            .collect::<Vec<_>>();
        let ids = self
            .queue
            .items()
            .iter()
            .filter(|item| !(item.kind == QueueKind::FollowUp && item.state == QueueState::Sent))
            .map(|item| (item.id.clone(), item.kind, item.state))
            .collect::<Vec<_>>();
        for (id, kind, state) in ids {
            if kind == QueueKind::Steer && state == QueueState::Sent {
                let control = control_of(&self.turn);
                if let Some(control) = control {
                    let id = id.clone();
                    tokio::spawn(async move {
                        drop(control.withdraw_steer(id).await);
                    });
                }
            }
            self.queue.remove(&id);
            if let Some(attachments) = self.attachments_by_id.remove(&id) {
                self.attachments.extend(attachments);
            }
        }
        if !restored.is_empty() {
            let mut text = restored.join("\n\n");
            if !self.composer.is_empty() {
                text = format!("{text}\n\n{}", self.composer.text());
            }
            self.composer.set_text(text);
        }
        self.interrupting = true;
    }

    fn handle_terminal(&mut self, event: Event, terminal: &mut TerminalSession) -> bool {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.handle_key(key, terminal);
                if let Some((text, copied)) = self.pending_copy.take() {
                    match terminal.copy(&text) {
                        Ok(()) => self.flash(copied),
                        Err(error) => self.flash(format!("copy failed: {error}")),
                    }
                }
                if std::mem::take(&mut self.toggle_mouse) {
                    let enabled = !terminal.mouse_capture();
                    drop(terminal.set_mouse_capture(enabled));
                    self.flash(if enabled {
                        "mouse scrolling and drag-to-copy on"
                    } else {
                        "mouse scrolling off (native selection)"
                    });
                }
                true
            }
            Event::Paste(text) => {
                if let Some(Overlay::Picker(_, picker)) = &mut self.overlay {
                    for character in text.chars().filter(|character| !character.is_control()) {
                        picker.handle_key(KeyEvent::new(
                            KeyCode::Char(character),
                            KeyModifiers::NONE,
                        ));
                    }
                } else if let Some(path) =
                    crate::extras::image_path(&text, &self.host.config.workspace)
                {
                    self.attach(path);
                } else {
                    self.composer.paste(&text);
                    self.update_finder();
                }
                true
            }
            Event::Mouse(mouse) => {
                if let Some(changed) = self.transcript_mouse(mouse, terminal) {
                    return changed;
                }
                let selected = match terminal.select(mouse) {
                    SelectionEvent::Ignored => false,
                    SelectionEvent::Changed => true,
                    SelectionEvent::Copied(_) => {
                        self.flash("copied the selection to the clipboard");
                        true
                    }
                    SelectionEvent::Failed(error) => {
                        self.flash(format!("copy failed: {error}"));
                        true
                    }
                };
                let handled = match mouse.kind {
                    MouseEventKind::Down(_) => self.click_pane(mouse.column, mouse.row),
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                        let transcript = self.transcript_at(mouse.column, mouse.row);
                        let previous = transcript.scroll_offset();
                        if mouse.kind == MouseEventKind::ScrollUp {
                            transcript.scroll_up(3);
                        } else {
                            transcript.scroll_down(3);
                        }
                        transcript.scroll_offset() != previous
                    }
                    _ => false,
                };
                selected || handled
            }
            Event::Resize(..) => true,
            _ => false,
        }
    }

    fn transcript_mouse(
        &mut self,
        mouse: MouseEvent,
        terminal: &mut TerminalSession,
    ) -> Option<bool> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if self.overlay.is_none() => {
                let side = self.pane_at(mouse.column, mouse.row)?;
                if let Some(previous) = self.dragging.take() {
                    self.pane_transcript(previous).select_cancel();
                }
                self.click_pane(mouse.column, mouse.row);
                if self
                    .pane_transcript(side)
                    .select_start(mouse.column, mouse.row)
                {
                    self.dragging = Some(side);
                }
                Some(true)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let side = self.dragging?;
                let transcript = self.pane_transcript(side);
                let changed = transcript.select_to(mouse.row);
                if transcript.autoscrolling() && self.autoscroll_at.is_none() {
                    self.autoscroll_at = Some(Instant::now() + AUTOSCROLL_STEP);
                }
                Some(changed)
            }
            MouseEventKind::Up(_) => {
                let side = self.dragging.take()?;
                self.autoscroll_at = None;
                let transcript = self.pane_transcript(side);
                let selected = transcript.select_end();
                let link = selected
                    .is_none()
                    .then(|| transcript.link_at(mouse.column, mouse.row))
                    .flatten();
                if let Some(text) = selected {
                    match terminal.copy(&text) {
                        Ok(()) => {
                            self.flash(format!("copied {} transcript lines", text.lines().count()))
                        }
                        Err(error) => self.flash(format!("copy failed: {error}")),
                    }
                }
                if let Some(url) = link {
                    self.flash(if crate::extras::open_url(&url) {
                        format!("opened {url}")
                    } else {
                        format!("could not open {url}")
                    });
                }
                Some(true)
            }
            MouseEventKind::Moved => {
                let side = self.dragging.take()?;
                self.autoscroll_at = None;
                self.pane_transcript(side).select_cancel();
                Some(true)
            }
            _ => None,
        }
    }

    fn autoscroll(&mut self) -> bool {
        self.dragging
            .is_some_and(|side| self.pane_transcript(side).autoscroll())
    }

    fn handle_key(&mut self, key: KeyEvent, terminal: &mut TerminalSession) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if self.overlay.is_some() {
            self.overlay_key(key);
            return;
        }
        let busy = !matches!(self.turn, TurnState::Idle);
        if key.code != KeyCode::Esc {
            self.esc_armed = None;
        }
        if self.finder.is_some() {
            match key.code {
                KeyCode::Up => {
                    if let Some(finder) = &mut self.finder {
                        finder.finder.select_previous();
                    }
                    return;
                }
                KeyCode::Down => {
                    if let Some(finder) = &mut self.finder {
                        finder.finder.select_next();
                    }
                    return;
                }
                KeyCode::Tab | KeyCode::Enter if key.modifiers.is_empty() => {
                    if self.accept_finder() || key.code == KeyCode::Tab {
                        return;
                    }
                }
                KeyCode::Esc => {
                    self.dismiss_finder();
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char('c')
                if control && self.panes.side_active() && self.composer.is_empty() =>
            {
                self.side_ctrl_c();
            }
            KeyCode::Char('c') if control => {
                if !self.composer.is_empty() {
                    self.stashed_draft = Some(self.composer.take());
                    self.flash("draft cleared · ctrl+z to restore");
                } else if self
                    .quit_armed
                    .is_some_and(|armed| armed.elapsed() < QUIT_CONFIRMATION)
                {
                    self.quit = true;
                } else {
                    self.quit_armed = Some(Instant::now());
                    self.flash(if busy {
                        "press ctrl+c again to quit · the turn continues when you resume · esc esc interrupts it"
                    } else {
                        "press ctrl+c again to quit"
                    });
                }
            }
            KeyCode::Char('z') if control => match self.stashed_draft.take() {
                Some(draft) => {
                    let current = self.composer.take();
                    self.composer.set_text(if current.is_empty() {
                        draft
                    } else {
                        format!("{draft}\n{current}")
                    });
                }
                None => self.flash("no cleared draft to restore"),
            },
            KeyCode::Esc if self.panes.side_active() => self.side_escape(),
            KeyCode::Esc => {
                if busy || self.focus.is_some() {
                    if self
                        .esc_armed
                        .is_some_and(|armed| armed.elapsed() < QUIT_CONFIRMATION)
                    {
                        self.esc_armed = None;
                        match self.focus {
                            Some(id) => self.interrupt_focused(id),
                            None => self.interrupt(),
                        }
                    } else {
                        self.esc_armed = Some(Instant::now());
                        self.flash("press esc again to interrupt");
                    }
                } else if self.queue.selected().is_some() {
                    self.queue.clear_selection();
                }
            }
            KeyCode::Char('d') if control => self.command("model"),
            KeyCode::Char('s') if control => self.command("thinking"),
            KeyCode::Char('o') if control => {
                let mode = self.active_transcript().cycle_mode();
                self.flash(format!("tools: {} · ctrl+o cycles", mode.label()));
            }
            KeyCode::Char('t') if control => self.show_tree = !self.show_tree,
            KeyCode::Char('r') if control => self.open_recent_prompts(),
            KeyCode::Char('g') if control => self.edit_externally(terminal),
            KeyCode::Char('l') if control => drop(terminal.clear()),
            KeyCode::Char('v') if control => self.paste_image(),
            KeyCode::F(1) => self.overlay = Some(Overlay::Help),
            KeyCode::PageUp => {
                let transcript = self.active_transcript();
                transcript.scroll_up(transcript.page());
            }
            KeyCode::PageDown => {
                let transcript = self.active_transcript();
                transcript.scroll_down(transcript.page());
            }
            KeyCode::Home if control => self.active_transcript().scroll_to_top(),
            KeyCode::End if control => self.active_transcript().scroll_to_bottom(),
            KeyCode::F(2) => self.cycle_focus(),
            KeyCode::Up if alt => self.recall_queued(),
            KeyCode::Char('u') if alt => self.recall_queued(),
            KeyCode::Tab
                if self.composer.is_empty()
                    && !self.panes.side_active()
                    && self.focus.is_none()
                    && self.recallable().is_some() =>
            {
                self.recall_queued();
            }
            KeyCode::Tab if self.panes.side.is_some() && self.composer.is_empty() => {
                self.toggle_pane();
            }
            KeyCode::BackTab
                if self.panes.side.is_some() && !self.composer.text().starts_with('/') =>
            {
                self.toggle_pane();
            }
            KeyCode::Tab
                if self.composer.text().starts_with('/') && !self.composer.text().contains(' ') =>
            {
                if let Some(command) = crate::commands::complete(
                    self.composer.text(),
                    self.host.config.clients.panel_command(),
                ) {
                    self.composer.set_text(format!("{command} "));
                }
            }
            KeyCode::Tab if self.complete_agent_argument() => {}
            KeyCode::Tab if busy && !self.composer.text().trim().is_empty() => {
                let text = self.composer.take();
                self.composer.history_mut().push(text.clone());
                self.quit_armed = None;
                self.submit(text, true);
            }
            KeyCode::Enter if busy && self.composer.is_empty() && key.modifiers.is_empty() => {
                self.promote_follow_up();
            }
            _ => match self.composer.handle_key(key) {
                ComposerAction::Submit(text) => {
                    self.quit_armed = None;
                    self.finder = None;
                    self.submit(text, false);
                }
                ComposerAction::Edited | ComposerAction::Moved => self.update_finder(),
                ComposerAction::Ignored => {}
            },
        }
    }

    fn promote_follow_up(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "promote_follow_up", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let latest = self
            .queue
            .items()
            .iter()
            .rev()
            .find(|item| {
                item.kind == QueueKind::FollowUp
                    && item.state == QueueState::Pending
                    && !self.resumes.contains_key(&item.id)
            })
            .map(|item| item.id.clone());
        match latest {
            Some(id) => {
                self.queue.convert(&id, QueueKind::Steer);
                self.flush_pending_steers();
            }
            None => self.flash("nothing queued to steer with"),
        }
    }

    fn open_recent_prompts(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "open_recent_prompts", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let mut seen = std::collections::HashSet::new();
        let items = self
            .composer
            .history()
            .entries()
            .iter()
            .rev()
            .filter(|entry| !entry.starts_with('/') && seen.insert(entry.as_str()))
            .map(|entry| PickerItem::choice(entry.replace('\n', " ⏎ ")))
            .collect::<Vec<_>>();
        if items.is_empty() {
            self.flash("no recent prompts");
            return;
        }
        let entries = self
            .composer
            .history()
            .entries()
            .iter()
            .rev()
            .filter(|entry| !entry.starts_with('/'))
            .fold(Vec::<String>::new(), |mut unique, entry| {
                if !unique.contains(entry) {
                    unique.push(entry.clone());
                }
                unique
            });
        let picker = Picker::new("Recent prompts", items)
            .filterable()
            .footer("type to filter · enter to edit · esc cancel");
        self.overlay = Some(Overlay::Picker(PickerPurpose::Recent(entries), picker));
    }

    fn edit_externally(&mut self, terminal: &mut TerminalSession) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "edit_externally", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".to_owned());
        let path = std::env::temp_dir().join(format!("nanocode-prompt-{}.md", std::process::id()));
        if let Err(error) = std::fs::write(&path, self.composer.text()) {
            self.notice(
                NoticeLevel::Error,
                format!("could not write {}: {error}", path.display()),
            );
            return;
        }
        let Some(words) = shlex_words(&editor) else {
            self.notice(
                NoticeLevel::Error,
                format!("could not parse editor command {editor:?}"),
            );
            return;
        };
        drop(terminal.suspend());
        let status = std::process::Command::new(&words[0])
            .args(&words[1..])
            .arg(&path)
            .status();
        drop(terminal.resume());
        match status {
            Ok(status) if status.success() => match std::fs::read_to_string(&path) {
                Ok(text) => self.composer.set_text(text.trim_end_matches('\n')),
                Err(error) => self.notice(
                    NoticeLevel::Error,
                    format!("could not read {}: {error}", path.display()),
                ),
            },
            Ok(status) => self.flash(format!("{editor} exited with {status}; draft unchanged")),
            Err(error) => self.notice(
                NoticeLevel::Error,
                format!("could not run {editor}: {error}"),
            ),
        }
        drop(std::fs::remove_file(&path));
    }

    fn recallable(&self) -> Option<&QueuedPrompt> {
        self.queue.items().iter().rev().find(|item| {
            !(item.kind == QueueKind::FollowUp && item.state == QueueState::Sent)
                && !self
                    .resumes
                    .get(&item.id)
                    .is_some_and(|(_, durable)| *durable)
        })
    }

    fn recall_queued(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "recall_queued", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(item) = self.recallable().cloned() else {
            self.flash("nothing queued");
            return;
        };
        if item.kind == QueueKind::Steer
            && item.state == QueueState::Sent
            && let Some(control) = control_of(&self.turn)
        {
            let id = item.id.clone();
            let tx = self.tx.clone();
            let text = item.text.clone();
            tokio::spawn(async move {
                match control.withdraw_steer(id).await {
                    Ok(true) => {}
                    Ok(false) => drop(tx.send(AppEvent::Notice(
                        NoticeLevel::Warning,
                        format!("steer was already delivered: {text}"),
                    ))),
                    Err(error) => drop(tx.send(AppEvent::Notice(
                        NoticeLevel::Warning,
                        format!("withdraw failed: {error}"),
                    ))),
                }
            });
        }
        self.queue.remove(&item.id);
        self.resumes.remove(&item.id);
        if let Some(attachments) = self.attachments_by_id.remove(&item.id) {
            self.attachments.extend(attachments);
        }
        let text = if self.composer.is_empty() {
            item.text
        } else {
            format!("{}\n{}", item.text, self.composer.text())
        };
        self.composer.set_text(text);
    }

    fn submit(&mut self, text: String, queue: bool) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "submit", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        self.append_history(&text);
        self.stashed_draft = None;
        if self.panes.side_active() {
            self.side_submit(text);
            return;
        }
        self.transcript.scroll_to_bottom();
        if let Some(command) = text.strip_prefix('/')
            && !command.starts_with('/')
        {
            self.command(command.trim());
            return;
        }
        if let Some(command) = text.strip_prefix('!')
            && !command.trim().is_empty()
        {
            self.run_shell(command.trim().to_owned());
            return;
        }
        if let Some(id) = self.focus {
            if !self.attachments.is_empty() {
                self.flash("attachments go to the main thread; /agent returns to it");
            }
            self.message_focused(id, text);
            return;
        }
        if self.moved {
            self.composer.set_text(text);
            self.flash(OWNER_MOVED);
            return;
        }
        let attachments = std::mem::take(&mut self.attachments);
        match self.turn {
            TurnState::Running { .. } | TurnState::Starting(_) | TurnState::External(_) => {
                let id = if queue {
                    self.enqueue_follow_up(text)
                } else {
                    self.enqueue_steer(text)
                };
                self.attachments_by_id.insert(id, attachments);
                if queue {
                    self.flash("queued for after this turn · tab on an empty prompt takes it back");
                } else {
                    self.flush_pending_steers();
                }
            }
            TurnState::Idle
                if self.agent.is_none() || self.pending_switch.is_some() || self.starting =>
            {
                let id = self.enqueue_follow_up(text);
                self.attachments_by_id.insert(id, attachments);
            }
            TurnState::Idle if self.hold_queue => {
                self.hold_queue = false;
                let id = self.enqueue_follow_up(text);
                self.attachments_by_id.insert(id, attachments);
                self.after_turn();
            }
            TurnState::Idle => {
                self.next_attachments = attachments;
                self.start_turn(text);
            }
        }
    }

    fn append_history(&self, text: &str) {
        if let Some(path) = &self.history_path {
            crate::history::append(path, text);
        }
    }

    pub(crate) fn deliver_to_main(&mut self, text: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "deliver_to_main", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        match self.turn {
            TurnState::Idle
                if self.agent.is_none() || self.pending_switch.is_some() || self.starting =>
            {
                self.enqueue_follow_up(text);
            }
            TurnState::Idle => self.start_turn(text),
            _ => {
                self.enqueue_steer(text);
                self.flush_pending_steers();
            }
        }
    }

    pub(crate) fn command(&mut self, command: &str) {
        let (name, argument) = command
            .split_once(' ')
            .map_or((command, ""), |(name, rest)| (name, rest.trim()));
        let _span = tracing::info_span!(target: "nanocode", "nc.command", command.name = name, session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        match name {
            "model" | "m" if self.session_started => {
                self.flash(
                    "the model is fixed for this thread; use /new before choosing another model",
                );
            }
            "model" | "m" if argument.is_empty() => {
                self.overlay = Some(Overlay::Picker(
                    PickerPurpose::Model,
                    models::model_picker(&self.model, self.host.config.gateway.as_ref()),
                ));
            }
            "model" | "m" => match crate::host::parse_model(argument).and_then(|model| match &self
                .host
                .config
                .gateway
            {
                Some(gateway) => gateway.check(&model).map(|()| model),
                None => Ok(model),
            }) {
                Ok(model) => {
                    let picker = models::thinking_picker(&model, self.thinking);
                    self.overlay = Some(Overlay::Picker(PickerPurpose::Thinking(model), picker));
                }
                Err(error) => self.notice(NoticeLevel::Error, error.to_string()),
            },
            "thinking" | "effort" | "reasoning" if argument.is_empty() => {
                let model = self.model.clone();
                self.open_dial(model, self.thinking, false);
            }
            "thinking" | "effort" | "reasoning" => match crate::host::parse_thinking(argument) {
                Ok(thinking) => self.request_switch(self.model.clone(), thinking, self.tier),
                Err(error) => self.notice(NoticeLevel::Error, error.to_string()),
            },
            "fast" | "tier" => {
                let capabilities = self.model.capabilities(nanocode::ModelTransport::Native);
                let tiers = capabilities.service_tiers().collect::<Vec<_>>();
                let tier = if argument.is_empty() {
                    let current = tiers
                        .iter()
                        .position(|tier| *tier == self.tier)
                        .unwrap_or(0);
                    tiers[(current + 1) % tiers.len()]
                } else {
                    match crate::host::parse_tier(argument) {
                        Ok(tier) => tier,
                        Err(error) => {
                            self.notice(NoticeLevel::Error, error.to_string());
                            return;
                        }
                    }
                };
                self.request_switch(self.model.clone(), self.thinking, tier);
            }
            "btw" => self.btw(argument.to_owned()),
            "collapse" => self.collapse_side(),
            "split" => self.split_side(),
            "close" => self.close_side(),
            "zoom" => self.zoom_pane(),
            "login" => self.login(argument),
            "resume" | "sessions" => self.open_session_picker(),
            "new" | "clear" => self.new_session(),
            "agents" => self.agents_command(argument),
            name if self
                .host
                .config
                .clients
                .panel_command()
                .is_some_and(|(command, _)| command.strip_prefix('/') == Some(name)) =>
            {
                self.show_panel = !self.show_panel;
            }
            "stop" | "stop-agent" => self.stop_command(argument),
            "compact" => self.compact(),
            "runner" => self.runner_command(argument),
            "session" | "status" => {
                let session = self.session_id().unwrap_or_else(|| "not started".into());
                let store = if self.host.config.store.is_some() {
                    "durable"
                } else {
                    "ephemeral"
                };
                let remote = self.runner_status();
                let route = self
                    .host
                    .config
                    .gateway
                    .as_ref()
                    .map_or_else(String::new, |gateway| format!(" · {}", gateway.describe()));
                self.notice(
                    NoticeLevel::Info,
                    format!(
                        "session {session} ({store}) · {} · thinking {} · {} · {} · {remote}{route}",
                        self.model,
                        self.thinking,
                        self.tier.as_str(),
                        self.host.config.workspace.display()
                    ),
                );
            }
            "id" => match self.session_id() {
                Some(session) => {
                    self.notice(NoticeLevel::Info, format!("session {session}"));
                    self.pending_copy = Some((session, "copied the session id to the clipboard"));
                }
                None => self.flash("no session yet; send a message to start one"),
            },
            "expand" => self.active_transcript().toggle_expanded(),
            "agent" | "focus" => self.focus_command(argument),
            "theme" if argument.is_empty() => self.open_theme_picker(),
            "theme" => self.set_theme(argument),
            "copy" => self.copy_last_reply(),
            "context" => self.context_report(),
            "skills" | "skill" => self.open_skills(),
            "image" | "paste" => self.paste_image(),
            "detach" => {
                let count = self.attachments.len();
                self.attachments.clear();
                self.flash(format!("removed {count} attachment(s)"));
            }
            "mouse" => self.toggle_mouse = true,
            "help" | "keys" | "?" => self.overlay = Some(Overlay::Help),
            "quit" | "exit" | "q" => self.quit = true,
            "reload" | "restart" => self.request_reload(true),
            other => self.notice(
                NoticeLevel::Warning,
                format!("unknown command /{other}; try /help"),
            ),
        }
    }

    pub fn runner_status(&self) -> String {
        let runners = self.host.runtime.list_runners();
        if runners.is_empty() && self.host.runtime.runner_servers().is_empty() {
            return "local tools".into();
        }
        let selected = self.agent.as_ref().and_then(|agent| {
            match self
                .host
                .runtime
                .runner_pool()
                .code_mode_executors()
                .selection(agent.session_id())
            {
                ExecutorSelection::Executor(name) => Some(name.to_string()),
                ExecutorSelection::Local => None,
            }
        });
        let mut lines =
            selected
                .map(|name| format!("exec on {name}"))
                .into_iter()
                .chain(runners.into_iter().map(|runner| {
                    format!("{} {}", runner.name, runner_status_label(&runner.status))
                }))
                .collect::<Vec<_>>();
        lines.extend(
            self.host
                .runtime
                .runner_servers()
                .iter()
                .map(|server| format!("listening {}", server.endpoint())),
        );
        lines.join(" · ")
    }

    fn runner_command(&mut self, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "runner_command", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(words) = shlex_words(argument) else {
            self.notice(NoticeLevel::Error, "unterminated quote in /runner command");
            return;
        };
        let parsed = crate::commands::RunnerCommand::try_parse_from(
            std::iter::once("/runner".to_owned()).chain(words),
        );
        let action = match parsed {
            Ok(command) => command
                .action
                .unwrap_or(crate::commands::RunnerAction::List),
            Err(error) => {
                let level = if error.use_stderr() {
                    NoticeLevel::Error
                } else {
                    NoticeLevel::Info
                };
                self.notice(level, error.to_string());
                return;
            }
        };
        let runtime = self.host.runtime.clone();
        let tx = self.tx.clone();
        match action {
            crate::commands::RunnerAction::List => {
                let mut lines = runtime
                    .list_runners()
                    .iter()
                    .map(runner_info)
                    .collect::<Vec<_>>();
                lines.extend(
                    runtime
                        .runner_servers()
                        .iter()
                        .map(|server| format!("listening {}", server.endpoint())),
                );
                self.notice(
                    NoticeLevel::Info,
                    if lines.is_empty() {
                        "no runners or listeners".into()
                    } else {
                        lines.join("\n")
                    },
                );
            }
            crate::commands::RunnerAction::Connect {
                url,
                token,
                name,
                replace,
            } => {
                let mut runner = Runner::connect(url);
                if let Some(token) = token {
                    runner = runner.token(token);
                }
                if let Some(name) = name {
                    runner = runner.name(name);
                }
                runner = if replace {
                    runner.replace()
                } else {
                    runner.alongside()
                };
                let session = self
                    .agent
                    .as_ref()
                    .map(|agent| agent.session_id().to_owned());
                tokio::spawn(async move {
                    let (level, message) = match runtime.connect_runner(runner).await {
                        Ok(handle) => {
                            let info = handle.info();
                            if let Some(session) = session
                                .filter(|_| info.protocol == RunnerProtocol::CodeMode && !replace)
                            {
                                runtime.runner_pool().code_mode_executors().select(
                                    &session,
                                    Some(ExecutorSelection::Executor(info.name.as_str().into())),
                                );
                            }
                            (
                                NoticeLevel::Info,
                                format!("connected runner {}", runner_info(&info)),
                            )
                        }
                        Err(error) => (
                            NoticeLevel::Error,
                            format!("runner connection failed: {error}"),
                        ),
                    };
                    drop(tx.send(AppEvent::Notice(level, message)));
                });
                self.flash("connecting runner…");
            }
            crate::commands::RunnerAction::Listen { address, token } => {
                let mut config = RunnerServerConfig::listen(address.to_string());
                if let Some(token) = token {
                    config = config.token(token);
                }
                tokio::spawn(async move {
                    let result = runtime
                        .listen_for_runners(config)
                        .await
                        .map_err(|error| error.to_string());
                    drop(tx.send(AppEvent::RunnerListening(result)));
                });
                self.flash("opening runner listener…");
            }
            crate::commands::RunnerAction::Select { name } => {
                let Some(session) = self
                    .agent
                    .as_ref()
                    .map(|agent| agent.session_id().to_owned())
                else {
                    self.notice(NoticeLevel::Error, "no active session to route");
                    return;
                };
                let executors = runtime.runner_pool().code_mode_executors();
                if let Some(error) = name
                    .as_deref()
                    .and_then(|name| executors.connected(name).err())
                {
                    self.notice(NoticeLevel::Error, error.to_string());
                    return;
                }
                let (selection, message) = match name {
                    None => (
                        ExecutorSelection::Local,
                        "exec cells run on this machine".to_owned(),
                    ),
                    Some(name) => (
                        ExecutorSelection::Executor(name.as_str().into()),
                        format!("exec cells run on {name}"),
                    ),
                };
                executors.select(&session, Some(selection));
                self.notice(NoticeLevel::Info, message);
            }
            crate::commands::RunnerAction::Disconnect { name } => {
                tokio::spawn(async move {
                    let (level, message) = match runtime.disconnect_runner(&name).await {
                        Ok(_) => (NoticeLevel::Info, format!("disconnected runner {name}")),
                        Err(error) => (
                            NoticeLevel::Error,
                            format!("runner disconnect failed: {error}"),
                        ),
                    };
                    drop(tx.send(AppEvent::Notice(level, message)));
                });
            }
            crate::commands::RunnerAction::StopListening => {
                let servers = runtime.runner_servers();
                tokio::spawn(async move {
                    for server in servers {
                        if let Err(error) = server.stop().await {
                            drop(tx.send(AppEvent::Notice(NoticeLevel::Error, error.to_string())));
                            return;
                        }
                    }
                    drop(tx.send(AppEvent::Notice(
                        NoticeLevel::Info,
                        "stopped runner listeners".into(),
                    )));
                });
            }
        }
    }

    fn overlay_key(&mut self, key: KeyEvent) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "overlay_key", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        let action = match overlay {
            Overlay::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::F(1)
                ) {
                    self.overlay = None;
                }
                return;
            }
            Overlay::Dial { dial, .. } => {
                match dial.handle_key(key) {
                    DialAction::Cancelled => self.overlay = None,
                    DialAction::Selected(index) => {
                        let Some(Overlay::Dial {
                            model,
                            levels,
                            then_tier,
                            ..
                        }) = self.overlay.take()
                        else {
                            return;
                        };
                        let Some(thinking) = levels.get(index).copied() else {
                            return;
                        };
                        if then_tier {
                            self.after_thinking(model, thinking);
                        } else {
                            self.request_switch(model, thinking, self.tier);
                        }
                    }
                    DialAction::Changed | DialAction::Ignored => {}
                }
                return;
            }
            Overlay::Picker(_, picker) => picker.handle_key(key),
        };
        match action {
            PickerAction::Cancelled => {
                let Some(Overlay::Picker(purpose, _)) = self.overlay.take() else {
                    return;
                };
                if matches!(purpose, PickerPurpose::Session(_))
                    && self.agent.is_none()
                    && !self.starting
                {
                    self.connect(None);
                }
            }
            PickerAction::Selected(index) => {
                let Some(Overlay::Picker(purpose, picker)) = self.overlay.take() else {
                    return;
                };
                self.picked(purpose, &picker, index);
            }
            PickerAction::Changed | PickerAction::Ignored => {}
        }
    }

    pub(crate) fn open_dial(&mut self, model: ModelSelection, thinking: Thinking, then_tier: bool) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "open_dial", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        match models::thinking_dial(&model, thinking, &self.theme) {
            Some((dial, levels)) => {
                self.overlay = Some(Overlay::Dial {
                    model,
                    levels,
                    then_tier,
                    dial,
                });
            }
            None => self.notice(
                NoticeLevel::Error,
                format!("{model} reports no thinking levels"),
            ),
        }
    }

    fn after_thinking(&mut self, model: ModelSelection, thinking: Thinking) {
        let tiers = models::tier_options(&model);
        if tiers.len() > 1 {
            let picker = models::tier_picker(&model, &tiers, self.tier);
            self.overlay = Some(Overlay::Picker(
                PickerPurpose::Tier(model, thinking),
                picker,
            ));
        } else {
            self.request_switch(model, thinking, ServiceTier::Standard);
        }
    }

    fn picked(&mut self, purpose: PickerPurpose, picker: &Picker, index: usize) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "picked", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        match purpose {
            PickerPurpose::Model => {
                let Some(model) = models::model_at(picker, index) else {
                    return;
                };
                let thinking = if model.supports_thinking(self.thinking) {
                    self.thinking
                } else {
                    model.default_thinking()
                };
                self.open_dial(model, thinking, true);
            }
            PickerPurpose::Thinking(model) => {
                let Some(thinking) = models::thinking_at(&model, picker, index) else {
                    return;
                };
                self.after_thinking(model, thinking);
            }
            PickerPurpose::Tier(model, thinking) => {
                let tiers = models::tier_options(&model);
                let Some(tier) = tiers.get(index).copied() else {
                    return;
                };
                self.request_switch(model, thinking, tier);
            }
            PickerPurpose::Theme => {
                if let Some(item) = picker.items().get(index) {
                    let name = item.label.clone();
                    self.set_theme(&name);
                }
            }
            PickerPurpose::Skill(names) => {
                if let Some(name) = names.get(index) {
                    self.composer.insert_str(&format!("Use the {name} skill: "));
                }
            }
            PickerPurpose::Recent(entries) => {
                if let Some(entry) = entries.get(index) {
                    self.composer.set_text(entry.clone());
                }
            }
            PickerPurpose::Session(sessions) => {
                let Some(session) = crate::sessions::session_at(&sessions, picker, index) else {
                    return;
                };
                self.resume(session.record.session_id.clone());
            }
        }
    }
}

fn control_of(turn: &TurnState) -> Option<TurnControl> {
    match turn {
        TurnState::Running { control, .. } => Some(control.clone()),
        _ => None,
    }
}

pub async fn apply_tier(agent: &Agent, model: &ModelSelection, tier: ServiceTier) -> Result<()> {
    if tier != ServiceTier::Standard || model.supports_fast_mode() {
        agent.set_service_tier(tier).await?;
    }
    Ok(())
}

pub async fn open_session(
    host: &Host,
    session_id: &str,
    thinking_override: Option<Thinking>,
    tier_override: Option<ServiceTier>,
) -> Result<Ready> {
    let store = host
        .config
        .store
        .as_ref()
        .ok_or("resume requires durability; remove --ephemeral")?;
    let stored = tracing::Instrument::instrument(
        store.load(session_id),
        tracing::info_span!(target: "nanocode", "nc.session.load", session.id = session_id),
    )
    .await?;
    let model = stored.summary.record.model.clone();
    let checkpoint = stored.session_checkpoint()?;
    let checkpoint_thinking = checkpoint.as_ref().map(SessionCheckpoint::thinking);
    let recorded_tier = checkpoint
        .as_ref()
        .and_then(SessionCheckpoint::service_tier)
        .or_else(|| {
            stored
                .summary
                .record
                .initial
                .map(|initial| initial.service_tier)
        });
    let opened_thinking = checkpoint_thinking.unwrap_or_else(|| model.default_thinking());
    let thinking = thinking_override
        .or(checkpoint_thinking)
        .or_else(|| {
            stored
                .summary
                .record
                .initial
                .map(|initial| initial.thinking)
        })
        .unwrap_or(opened_thinking);
    model
        .capabilities(nanocode::ModelTransport::Native)
        .check_thinking(thinking)?;
    let pending = pending_turns(&stored.turns, stored.in_progress.as_deref())?;
    let prompts = stored
        .turns
        .iter()
        .filter(|turn| turn.status != nanocode::durability::TurnStatus::Pending)
        .filter_map(|turn| turn.preview.clone())
        .collect();
    let (agent, events) = tracing::Instrument::instrument(
        host.open(store, &stored),
        tracing::info_span!(target: "nanocode", "nc.session.restore", session.id = session_id),
    )
    .await?;
    if thinking != opened_thinking {
        agent.set_thinking(thinking).await?;
    }
    let tier = match tier_override {
        Some(tier) => {
            model
                .capabilities(nanocode::ModelTransport::Native)
                .check_service_tier(tier)?;
            apply_tier(&agent, &model, tier).await?;
            tier
        }
        None => recorded_tier.unwrap_or_default(),
    };
    Ok(Ready {
        notice: format!("resumed session {session_id} · {model} · thinking {thinking}"),
        agent,
        events,
        model,
        thinking,
        tier,
        history: Some(stored.transcript),
        reset_transcript: true,
        pending,
        prompts,
    })
}

const OWNER_MOVED: &str = "this session was opened in another nc window, which now owns it; this window stopped writing so the two cannot conflict. /resume here takes it back and stops the other window";

fn report_owner_moved(
    tx: &UnboundedSender<AppEvent>,
    generation: u64,
    error: &nanocode::NanocodeError,
) {
    if owner_moved(error) {
        drop(tx.send(AppEvent::OwnerMoved { generation }));
    }
}

fn owner_moved(error: &nanocode::NanocodeError) -> bool {
    use nanocode::durability::{Error, StoreError};
    match error {
        nanocode::NanocodeError::ExecutionPolicy { source, .. } => {
            source.downcast_ref::<Error>().is_some_and(|error| {
                matches!(
                    error,
                    Error::ModelOwnerFenced | Error::Store(StoreError::Fenced)
                )
            }) || source
                .downcast_ref::<nanocode::NanocodeError>()
                .is_some_and(owner_moved)
        }
        nanocode::NanocodeError::Shutdown(inner) => owner_moved(inner),
        _ => false,
    }
}

fn operation_active(error: &nanocode::NanocodeError) -> bool {
    matches!(
        error,
        nanocode::NanocodeError::ExecutionPolicy { source, .. }
            if matches!(
                source.downcast_ref::<nanocode::durability::Error>(),
                Some(nanocode::durability::Error::OperationActive { .. })
            )
    )
}

fn pending_turns(
    turns: &[nanocode::durability::StoredTurn],
    in_progress: Option<&str>,
) -> Result<Vec<PendingTurn>> {
    turns
        .iter()
        .filter(|turn| turn.status == nanocode::durability::TurnStatus::Pending)
        .map(|turn| {
            let input = turn.input.get("prompt").unwrap_or(&turn.input).clone();
            let prompt = serde_json::from_value::<nanocode::agent::input::Prompt>(input).map_err(
                |error| format!("pending turn {} has an unreadable prompt: {error}", turn.id),
            )?;
            Ok::<_, crate::host::Error>(PendingTurn {
                id: turn.id.clone(),
                text: turn.preview.clone().unwrap_or_default(),
                prompt,
                shown: in_progress == Some(turn.id.as_str()),
            })
        })
        .collect()
}

pub fn subagent_status(status: &AgentStatus) -> (NodeStatus, String) {
    match status {
        AgentStatus::Pending => (NodeStatus::Pending, String::new()),
        AgentStatus::Running => (NodeStatus::Running, String::new()),
        AgentStatus::Completed { output } => {
            let text = match output {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            (
                NodeStatus::Completed,
                text.lines().next().unwrap_or_default().to_owned(),
            )
        }
        AgentStatus::Interrupted => (NodeStatus::Idle, "interrupted".into()),
        AgentStatus::Failed { error } => (NodeStatus::Failed, error.clone()),
        AgentStatus::Closing | AgentStatus::Closed => (NodeStatus::Closed, String::new()),
    }
}

pub fn family_label(family: &HarnessFamily) -> &'static str {
    if *family == HarnessFamily::Claude {
        "Claude"
    } else {
        "Codex"
    }
}

fn shlex_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut started = false;
    for character in command.chars() {
        match (quote, character) {
            (Some(open), character) if character == open => quote = None,
            (Some(_), character) => current.push(character),
            (None, '"' | '\'') => {
                quote = Some(character);
                started = true;
            }
            (None, character) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, character) => {
                current.push(character);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        words.push(current);
    }
    (!words.is_empty()).then_some(words)
}
