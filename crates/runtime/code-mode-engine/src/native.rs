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

use rquickjs::{Context, Ctx, Runtime};
use serde_json::{Value, value::RawValue};
use tokio::sync::mpsc;

use crate::cell::{
    Rejections, RuntimeEvent, clear_pending, completed_terminal, drain_jobs, invoke_timeout,
    new_state, next_due_timeout, next_timeout_wait, resolve_tool, send_terminal, start_execution,
    track_rejections,
};

pub struct EmbeddedHost {
    command_tx: std_mpsc::Sender<HostCommand>,
    events: mpsc::UnboundedReceiver<RuntimeEvent>,
    interrupted: Arc<AtomicBool>,
    startup: Arc<OnceLock<String>>,
    worker: Option<thread::JoinHandle<()>>,
}

struct StartExecution {
    execution_id: u64,
    source: String,
    tools: Vec<Value>,
    stored: HashMap<String, Arc<RawValue>>,
    prelude: Option<Arc<str>>,
}

enum HostCommand {
    Start(StartExecution),
    ToolResult {
        execution_id: u64,
        id: u64,
        encoded: String,
        success: bool,
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
        let worker = thread::Builder::new()
            .name("nanocode-code-mode-quickjs".to_owned())
            .spawn(move || {
                run_worker(&command_rx, &event_tx, &worker_startup, &worker_interrupted);
            })
            .map_err(|error| format!("failed to start embedded QuickJS code-mode host: {error}"))?;
        Ok(Self {
            command_tx,
            events,
            interrupted,
            startup,
            worker: Some(worker),
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
    ) -> Result<(), String> {
        self.command_tx
            .send(HostCommand::Start(StartExecution {
                execution_id,
                source: source.to_owned(),
                tools,
                stored,
                prelude,
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

    pub async fn terminate(&mut self) {
        self.interrupted.store(true, Ordering::Release);
        let _ = self.command_tx.send(HostCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = tokio::task::spawn_blocking(move || worker.join()).await;
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
    let rejections = track_rejections(&runtime);
    let interrupt_signal = Arc::clone(interrupted);
    runtime.set_interrupt_handler(Some(Box::new(move || {
        interrupt_signal.load(Ordering::Acquire)
    })));
    let prewarmed_context = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => {
            let _ = startup.set(format!(
                "failed to create embedded QuickJS context: {error}"
            ));
            return;
        }
    };
    drop(prewarmed_context);
    runtime.run_gc();

    loop {
        match command_rx.recv() {
            Ok(HostCommand::Start(start)) => {
                interrupted.store(false, Ordering::Release);
                let context = match Context::full(&runtime) {
                    Ok(context) => context,
                    Err(error) => {
                        tracing::error!(
                            target: "nanocode_code_mode",
                            %error,
                            "failed to create a fresh embedded QuickJS context"
                        );
                        return;
                    }
                };
                let result =
                    run_execution(&context, start, command_rx, event_tx.clone(), &rejections);
                drop(context);
                runtime.run_gc();
                if result.is_err() {
                    return;
                }
            }
            Ok(HostCommand::ToolResult { .. }) => {}
            Ok(HostCommand::Shutdown) | Err(_) => return,
        }
    }
}

fn run_execution(
    context: &Context,
    start: StartExecution,
    command_rx: &std_mpsc::Receiver<HostCommand>,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    rejections: &Rejections,
) -> Result<(), String> {
    context.with(|ctx| run_execution_in_context(&ctx, start, command_rx, event_tx, rejections))
}

fn run_execution_in_context(
    ctx: &Ctx<'_>,
    start: StartExecution,
    command_rx: &std_mpsc::Receiver<HostCommand>,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    rejections: &Rejections,
) -> Result<(), String> {
    let execution_id = start.execution_id;
    let state = new_state(execution_id, event_tx, start.stored, Rc::clone(rejections));
    let promise = start_execution(
        ctx,
        &state,
        start.source,
        start.tools,
        start.prelude.as_deref(),
    )?;

    let result = (|| {
        loop {
            if let Some(terminal) = completed_terminal(ctx, &promise)? {
                return send_terminal(&state, terminal);
            }
            match receive_command(command_rx, next_timeout_wait(&state)) {
                CommandPoll::Command(HostCommand::ToolResult {
                    execution_id: result_execution_id,
                    id,
                    encoded,
                    success,
                }) if result_execution_id == execution_id => {
                    resolve_tool(ctx, &state, id, encoded, success)?;
                    drain_jobs(ctx, &state);
                }
                CommandPoll::TimerReady => {
                    if let Some(id) = next_due_timeout(&state) {
                        invoke_timeout(ctx, &state, id)?;
                        drain_jobs(ctx, &state);
                    }
                }
                CommandPoll::Command(HostCommand::Shutdown) | CommandPoll::Disconnected => {
                    return Err("embedded QuickJS host stopped".into());
                }
                CommandPoll::Command(HostCommand::Start(_) | HostCommand::ToolResult { .. }) => {}
            }
        }
    })();
    clear_pending(&state);
    result
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
