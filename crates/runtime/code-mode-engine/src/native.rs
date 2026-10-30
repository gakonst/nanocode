use std::{
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::Duration,
};

use nanocode_platform::Instant;
use rquickjs::{CatchResultExt, Context, Ctx, Function, Persistent, Runtime};
use serde_json::{Value, value::RawValue};
use tokio::sync::{mpsc, oneshot};

use crate::{
    cell::{
        ExecutionState, Rejections, RuntimeEvent, begin_execution, clear_pending,
        completed_terminal, drain_jobs, install_bootstrap, install_prelude, invoke_timeout,
        new_state, next_due_timeout, next_timeout_wait, prepare_execution, replay_inputs,
        resolve_tool, send_terminal, start_execution, track_rejections,
    },
    journal::CellJournaling,
};

struct Prepared {
    run_cell: Persistent<Function<'static>>,
    state: Rc<std::cell::RefCell<ExecutionState>>,
    context: Context,
    bytes: bool,
}

impl Prepared {
    fn new(
        runtime: &Runtime,
        event_tx: &mpsc::UnboundedSender<RuntimeEvent>,
        rejections: &Rejections,
        bytes: bool,
    ) -> Result<Self, String> {
        let context = Context::full(runtime).map_err(|error| {
            format!("failed to create a fresh embedded QuickJS context: {error}")
        })?;
        let state = new_state(0, event_tx.clone(), HashMap::new(), Rc::clone(rejections));
        let run_cell = context.with(|ctx| {
            prepare_execution(&ctx, &state, bytes).map(|run_cell| Persistent::save(&ctx, run_cell))
        });
        match run_cell {
            Ok(run_cell) => Ok(Self {
                run_cell,
                state,
                context,
                bytes,
            }),
            Err(error) => {
                clear_pending(&state);
                drop(state);
                drop(context);
                Err(error)
            }
        }
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        clear_pending(&self.state);
    }
}

pub struct EmbeddedHost {
    command_tx: std_mpsc::Sender<HostCommand>,
    events: mpsc::UnboundedReceiver<RuntimeEvent>,
    interrupted: Arc<AtomicBool>,
    startup: Arc<OnceLock<String>>,
    stopped: Option<oneshot::Receiver<()>>,
}

struct StartExecution {
    execution_id: u64,
    source: String,
    tools: Vec<Value>,
    stored: HashMap<String, Arc<RawValue>>,
    prelude: Option<Arc<str>>,
    journaling: Option<CellJournaling>,
}

enum HostCommand {
    Start(StartExecution),
    ToolResult {
        execution_id: u64,
        id: u64,
        encoded: String,
        success: bool,
    },
    Hold {
        execution_id: u64,
    },
    Release {
        execution_id: u64,
    },
    Shutdown,
}

enum CommandPoll {
    Command(HostCommand),
    TimerReady,
    Disconnected,
}

impl EmbeddedHost {
    pub fn spawn() -> Result<Self, String> {
        let (command_tx, command_rx) = std_mpsc::channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let interrupted = Arc::new(AtomicBool::new(false));
        let startup = Arc::new(OnceLock::new());
        let worker_interrupted = Arc::clone(&interrupted);
        let worker_startup = Arc::clone(&startup);
        let (stopped_tx, stopped) = oneshot::channel();
        thread::Builder::new()
            .name("nanocode-code-mode-quickjs".to_owned())
            .spawn(move || {
                run_worker(&command_rx, &event_tx, &worker_startup, &worker_interrupted);
                drop((command_rx, event_tx));
                let _ = stopped_tx.send(());
            })
            .map_err(|error| format!("failed to start embedded QuickJS code-mode host: {error}"))?;
        Ok(Self {
            command_tx,
            events,
            interrupted,
            startup,
            stopped: Some(stopped),
        })
    }

    fn unavailable(&self, fallback: &str) -> String {
        self.startup
            .get()
            .cloned()
            .unwrap_or_else(|| fallback.to_owned())
    }

    pub fn start_cell(
        &self,
        execution_id: u64,
        source: &str,
        stored: HashMap<String, Arc<RawValue>>,
        tools: Vec<Value>,
        prelude: Option<Arc<str>>,
        journaling: Option<CellJournaling>,
    ) -> Result<(), String> {
        self.command_tx
            .send(HostCommand::Start(StartExecution {
                execution_id,
                source: source.to_owned(),
                tools,
                stored,
                prelude,
                journaling,
            }))
            .map_err(|_| self.unavailable("embedded QuickJS code-mode host is unavailable"))
    }

    pub async fn read_event(&mut self) -> Result<RuntimeEvent, String> {
        match self.events.recv().await {
            Some(event) => Ok(event),
            None => Err(self.unavailable("embedded QuickJS code-mode host ended before a result")),
        }
    }

    pub fn send_tool_result(
        &self,
        execution_id: u64,
        id: u64,
        encoded: String,
        success: bool,
    ) -> Result<(), String> {
        self.command_tx
            .send(HostCommand::ToolResult {
                execution_id,
                id,
                encoded,
                success,
            })
            .map_err(|_| {
                "embedded QuickJS code-mode host closed before accepting a tool result".into()
            })
    }

    pub fn hold(&self, execution_id: u64) -> Result<(), String> {
        self.command_tx
            .send(HostCommand::Hold { execution_id })
            .map_err(|_| "embedded QuickJS code-mode host closed before holding a cell".into())
    }

    pub fn release(&self, execution_id: u64) -> Result<(), String> {
        self.command_tx
            .send(HostCommand::Release { execution_id })
            .map_err(|_| "embedded QuickJS code-mode host closed before releasing a cell".into())
    }

    pub async fn terminate(&mut self) {
        self.interrupted.store(true, Ordering::Release);
        let _ = self.command_tx.send(HostCommand::Shutdown);
        if let Some(stopped) = self.stopped.take() {
            let _ = stopped.await;
        }
    }
}

impl Drop for EmbeddedHost {
    fn drop(&mut self) {
        self.interrupted.store(true, Ordering::Release);
        let _ = self.command_tx.send(HostCommand::Shutdown);
    }
}

fn run_worker(
    command_rx: &std_mpsc::Receiver<HostCommand>,
    event_tx: &mpsc::UnboundedSender<RuntimeEvent>,
    startup: &OnceLock<String>,
    interrupted: &Arc<AtomicBool>,
) {
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = startup.set(format!(
                "failed to initialize embedded QuickJS runtime: {error}"
            ));
            return;
        }
    };
    if let Err(error) = install_bootstrap(&runtime) {
        let _ = startup.set(error);
        return;
    }
    let rejections = track_rejections(&runtime);
    let interrupt_signal = Arc::clone(interrupted);
    runtime.set_interrupt_handler(Some(Box::new(move || {
        interrupt_signal.load(Ordering::Acquire)
    })));
    let mut prepared: Option<Prepared> = None;
    loop {
        match command_rx.recv() {
            Ok(HostCommand::Start(start)) => {
                interrupted.store(false, Ordering::Release);
                if let Err(error) = install_prelude(&runtime, start.prelude.as_deref()) {
                    let _ = event_tx.send(RuntimeEvent::Error {
                        cell_id: start.execution_id,
                        message: error,
                        stored: HashMap::new(),
                    });
                    continue;
                }
                let bytes = start.prelude.is_some();
                let next = match prepared.take() {
                    Some(next) if next.bytes == bytes => Ok(next),
                    stale => {
                        drop(stale);
                        Prepared::new(&runtime, event_tx, &rejections, bytes)
                    }
                };
                let next = match next {
                    Ok(next) => next,
                    Err(error) => {
                        tracing::error!(
                            target: "nanocode_code_mode",
                            %error,
                            "failed to create a fresh embedded QuickJS context"
                        );
                        return;
                    }
                };
                let result = run_execution(&next, start, command_rx);
                drop(next);
                runtime.run_gc();
                if result.is_err() {
                    return;
                }
                prepared = Prepared::new(&runtime, event_tx, &rejections, bytes).ok();
            }
            Ok(
                HostCommand::ToolResult { .. }
                | HostCommand::Hold { .. }
                | HostCommand::Release { .. },
            ) => {}
            Ok(HostCommand::Shutdown) | Err(_) => return,
        }
    }
}

fn run_execution(
    prepared: &Prepared,
    start: StartExecution,
    command_rx: &std_mpsc::Receiver<HostCommand>,
) -> Result<(), String> {
    prepared
        .context
        .with(|ctx| run_execution_in_context(&ctx, prepared, start, command_rx))
}

fn run_execution_in_context(
    ctx: &Ctx<'_>,
    prepared: &Prepared,
    start: StartExecution,
    command_rx: &std_mpsc::Receiver<HostCommand>,
) -> Result<(), String> {
    let execution_id = start.execution_id;
    let state = &prepared.state;
    begin_execution(state, execution_id, start.stored, start.journaling);
    let run_cell = prepared
        .run_cell
        .clone()
        .restore(ctx)
        .catch(ctx)
        .map_err(|error| format!("failed to restore the QuickJS bootstrap: {error}"))?;
    let promise = start_execution(
        ctx,
        state,
        &run_cell,
        start.source,
        start.tools,
        start.prelude.is_some(),
    )?;

    let result = (|| {
        if let Some(reason) = replay_inputs(ctx, state, &promise)? {
            return state
                .borrow()
                .send(RuntimeEvent::Diverged {
                    cell_id: execution_id,
                    reason,
                })
                .map_err(|_| "embedded QuickJS execution observer closed".to_owned());
        }
        loop {
            if let Some(terminal) = completed_terminal(ctx, &promise)? {
                return send_terminal(state, terminal);
            }
            match receive_command(command_rx, next_timeout_wait(state)) {
                CommandPoll::Command(HostCommand::ToolResult {
                    execution_id: result_execution_id,
                    id,
                    encoded,
                    success,
                }) if result_execution_id == execution_id => {
                    resolve_tool(ctx, state, id, encoded, success)?;
                    drain_jobs(ctx, state);
                }
                CommandPoll::TimerReady => {
                    let now = Instant::now();
                    while let Some(id) = next_due_timeout(state, now) {
                        invoke_timeout(ctx, state, id)?;
                        drain_jobs(ctx, state);
                        if let Some(terminal) = completed_terminal(ctx, &promise)? {
                            return send_terminal(state, terminal);
                        }
                    }
                }
                CommandPoll::Command(HostCommand::Hold { execution_id: held })
                    if held == execution_id =>
                {
                    hold_execution(ctx, state, execution_id, command_rx)?;
                }
                CommandPoll::Command(HostCommand::Shutdown) | CommandPoll::Disconnected => {
                    return Err("embedded QuickJS host stopped".into());
                }
                CommandPoll::Command(
                    HostCommand::Start(_)
                    | HostCommand::ToolResult { .. }
                    | HostCommand::Hold { .. }
                    | HostCommand::Release { .. },
                ) => {}
            }
        }
    })();
    clear_pending(state);
    result
}

fn hold_execution(
    ctx: &Ctx<'_>,
    state: &Rc<std::cell::RefCell<ExecutionState>>,
    execution_id: u64,
    command_rx: &std_mpsc::Receiver<HostCommand>,
) -> Result<(), String> {
    state
        .borrow()
        .send(RuntimeEvent::Held {
            cell_id: execution_id,
        })
        .map_err(|_| "embedded QuickJS execution observer closed".to_owned())?;
    let mut stashed = Vec::new();
    loop {
        match command_rx.recv() {
            Ok(HostCommand::Release {
                execution_id: released,
            }) if released == execution_id => break,
            Ok(HostCommand::ToolResult {
                execution_id: result_execution_id,
                id,
                encoded,
                success,
            }) if result_execution_id == execution_id => stashed.push((id, encoded, success)),
            Ok(HostCommand::Shutdown) | Err(_) => {
                return Err("embedded QuickJS host stopped".into());
            }
            Ok(_) => {}
        }
    }
    for (id, encoded, success) in stashed {
        resolve_tool(ctx, state, id, encoded, success)?;
        drain_jobs(ctx, state);
    }
    Ok(())
}

fn receive_command(
    receiver: &std_mpsc::Receiver<HostCommand>,
    timeout: Option<Duration>,
) -> CommandPoll {
    let Some(timeout) = timeout else {
        return receiver
            .recv()
            .map_or(CommandPoll::Disconnected, CommandPoll::Command);
    };
    match receiver.recv_timeout(timeout) {
        Ok(command) => CommandPoll::Command(command),
        Err(std_mpsc::RecvTimeoutError::Timeout) => CommandPoll::TimerReady,
        Err(std_mpsc::RecvTimeoutError::Disconnected) => CommandPoll::Disconnected,
    }
}
