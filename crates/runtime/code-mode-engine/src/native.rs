use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering},
        mpsc as std_mpsc,
    },
    thread,
};

use nanocode_platform::Instant;
use rquickjs::{CatchResultExt, Context, Ctx, Function, Persistent, Promise, Runtime};
use serde_json::value::RawValue;
use tokio::sync::{mpsc, oneshot};

use crate::{
    cell::{
        CellTools, ExecutionState, Rejections, RuntimeEvent, begin_execution, clear_pending,
        completed_terminal, drain_jobs, install_bootstrap, install_prelude, invoke_timeout,
        new_state, next_due_timeout, next_timeout_wait, prepare_execution, replay_inputs,
        resolve_tool, send_terminal, start_execution, track_rejections,
    },
    journal::CellJournaling,
};

struct Prepared {
    run_cell: Persistent<Function<'static>>,
    state: Rc<RefCell<ExecutionState>>,
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

struct Engine {
    prepared: Option<Prepared>,
    rejections: Rejections,
    interrupted: Rc<RefCell<Arc<AtomicBool>>>,
    runtime: Runtime,
}

impl Engine {
    fn new(interrupted: &Arc<AtomicBool>) -> Result<Self, String> {
        let runtime = Runtime::new()
            .map_err(|error| format!("failed to initialize embedded QuickJS runtime: {error}"))?;
        install_bootstrap(&runtime)?;
        let rejections = track_rejections(&runtime);
        let interrupted = Rc::new(RefCell::new(Arc::clone(interrupted)));
        let signal = Rc::clone(&interrupted);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            signal.borrow().load(Ordering::Acquire)
        })));
        Ok(Self {
            prepared: None,
            rejections,
            interrupted,
            runtime,
        })
    }

    fn prepare(
        &mut self,
        event_tx: &mpsc::UnboundedSender<RuntimeEvent>,
        bytes: bool,
    ) -> Result<Prepared, String> {
        match self.prepared.take() {
            Some(prepared) if prepared.bytes == bytes => Ok(prepared),
            stale => {
                drop(stale);
                Prepared::new(&self.runtime, event_tx, &self.rejections, bytes)
            }
        }
    }
}

struct Execution {
    execution_id: u64,
    promise: Persistent<Promise<'static>>,
    prepared: Prepared,
    held: Option<Vec<(u64, String, bool)>>,
}

struct StartExecution {
    execution_id: u64,
    source: String,
    tools: Arc<CellTools>,
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
}

enum Message {
    Open {
        host: u64,
        claimed: bool,
        events: mpsc::UnboundedSender<RuntimeEvent>,
        interrupted: Arc<AtomicBool>,
    },
    Command {
        host: u64,
        command: HostCommand,
    },
    Close {
        host: u64,
        stopped: Option<oneshot::Sender<()>>,
    },
}

struct Host {
    execution: Option<Execution>,
    engine: Option<Engine>,
    events: mpsc::UnboundedSender<RuntimeEvent>,
    interrupted: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

enum Step {
    Pending,
    Finished,
}

struct WorkerLoad {
    live: AtomicUsize,
    spares: AtomicIsize,
    busy: AtomicBool,
}

struct WorkerHandle {
    sender: std_mpsc::Sender<Message>,
    load: Arc<WorkerLoad>,
}

struct Pool {
    workers: Vec<WorkerHandle>,
}

static NEXT_HOST: AtomicU64 = AtomicU64::new(1);

const SPARE_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

fn pool() -> Result<&'static Pool, String> {
    static POOL: OnceLock<Result<Pool, String>> = OnceLock::new();
    POOL.get_or_init(start_pool).as_ref().map_err(Clone::clone)
}

fn start_pool() -> Result<Pool, String> {
    let count = thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let mut workers = Vec::with_capacity(count);
    for index in 0..count {
        let (sender, receiver) = std_mpsc::channel();
        let load = Arc::new(WorkerLoad {
            live: AtomicUsize::new(0),
            spares: AtomicIsize::new(0),
            busy: AtomicBool::new(false),
        });
        let worker_load = Arc::clone(&load);
        thread::Builder::new()
            .name(format!("nanocode-code-mode-quickjs-{index}"))
            .spawn(move || Worker::new(worker_load).run(&receiver))
            .map_err(|error| {
                format!("failed to start embedded QuickJS code-mode worker: {error}")
            })?;
        workers.push(WorkerHandle { sender, load });
    }
    Ok(Pool { workers })
}

impl Pool {
    fn pick(&self) -> (&WorkerHandle, bool) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let start = NEXT.fetch_add(1, Ordering::Relaxed);
        let count = self.workers.len();
        for offset in 0..count {
            let worker = &self.workers[(start + offset) % count];
            if !worker.load.busy.load(Ordering::Relaxed)
                && worker
                    .load
                    .spares
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |spares| {
                        (spares > 0).then(|| spares - 1)
                    })
                    .is_ok()
            {
                return (worker, true);
            }
        }
        let worker = self
            .workers
            .iter()
            .min_by_key(|worker| {
                (
                    worker.load.busy.load(Ordering::Relaxed),
                    worker.load.live.load(Ordering::Relaxed),
                )
            })
            .expect("the QuickJS pool has at least one worker");
        (worker, false)
    }
}

pub struct EmbeddedHost {
    host: u64,
    worker: std_mpsc::Sender<Message>,
    events: mpsc::UnboundedReceiver<RuntimeEvent>,
    interrupted: Arc<AtomicBool>,
    closed: bool,
}

impl EmbeddedHost {
    pub fn spawn() -> Result<Self, String> {
        let (worker, claimed) = pool()?.pick();
        let (event_tx, events) = mpsc::unbounded_channel();
        let interrupted = Arc::new(AtomicBool::new(false));
        let host = NEXT_HOST.fetch_add(1, Ordering::Relaxed);
        worker.load.live.fetch_add(1, Ordering::Relaxed);
        worker
            .sender
            .send(Message::Open {
                host,
                claimed,
                events: event_tx,
                interrupted: Arc::clone(&interrupted),
            })
            .map_err(|_| "embedded QuickJS code-mode worker is unavailable".to_owned())?;
        Ok(Self {
            host,
            worker: worker.sender.clone(),
            events,
            interrupted,
            closed: false,
        })
    }

    pub async fn spawn_available() -> Result<Self, String> {
        Self::spawn()
    }

    fn command(&self, command: HostCommand, failure: &str) -> Result<(), String> {
        if self.events.is_closed() {
            return Err(failure.to_owned());
        }
        self.worker
            .send(Message::Command {
                host: self.host,
                command,
            })
            .map_err(|_| failure.to_owned())
    }

    pub fn start_cell(
        &self,
        execution_id: u64,
        source: &str,
        stored: HashMap<String, Arc<RawValue>>,
        tools: Arc<CellTools>,
        prelude: Option<Arc<str>>,
        journaling: Option<CellJournaling>,
    ) -> Result<(), String> {
        self.command(
            HostCommand::Start(StartExecution {
                execution_id,
                source: source.to_owned(),
                tools,
                stored,
                prelude,
                journaling,
            }),
            "embedded QuickJS code-mode host is unavailable",
        )
    }

    pub async fn read_event(&mut self) -> Result<RuntimeEvent, String> {
        self.events
            .recv()
            .await
            .ok_or_else(|| "embedded QuickJS code-mode host ended before a result".to_owned())
    }

    pub fn send_tool_result(
        &self,
        execution_id: u64,
        id: u64,
        encoded: String,
        success: bool,
    ) -> Result<(), String> {
        self.command(
            HostCommand::ToolResult {
                execution_id,
                id,
                encoded,
                success,
            },
            "embedded QuickJS code-mode host closed before accepting a tool result",
        )
    }

    pub fn hold(&self, execution_id: u64) -> Result<(), String> {
        self.command(
            HostCommand::Hold { execution_id },
            "embedded QuickJS code-mode host closed before holding a cell",
        )
    }

    pub fn release(&self, execution_id: u64) -> Result<(), String> {
        self.command(
            HostCommand::Release { execution_id },
            "embedded QuickJS code-mode host closed before releasing a cell",
        )
    }

    pub async fn terminate(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.interrupted.store(true, Ordering::Release);
        let (stopped, done) = oneshot::channel();
        if self
            .worker
            .send(Message::Close {
                host: self.host,
                stopped: Some(stopped),
            })
            .is_ok()
        {
            let _ = done.await;
        }
    }
}

impl Drop for EmbeddedHost {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        self.interrupted.store(true, Ordering::Release);
        let _ = self.worker.send(Message::Close {
            host: self.host,
            stopped: None,
        });
    }
}

struct Worker {
    load: Arc<WorkerLoad>,
    hosts: HashMap<u64, Host>,
    deadlines: BTreeSet<(Instant, u64)>,
    spares: Vec<Engine>,
    opened: usize,
    demand: usize,
    window: Instant,
    idle_events: mpsc::UnboundedSender<RuntimeEvent>,
}

impl Worker {
    fn new(load: Arc<WorkerLoad>) -> Self {
        Self {
            load,
            hosts: HashMap::new(),
            deadlines: BTreeSet::new(),
            spares: Vec::new(),
            opened: 0,
            demand: 0,
            window: Instant::now(),
            idle_events: mpsc::unbounded_channel().0,
        }
    }

    fn run(mut self, receiver: &std_mpsc::Receiver<Message>) {
        loop {
            let trim = (self.spares.len() > 1).then(|| self.window + SPARE_WINDOW);
            let wake = match (self.deadlines.first().map(|(deadline, _)| *deadline), trim) {
                (Some(deadline), Some(trim)) => Some(deadline.min(trim)),
                (deadline, trim) => deadline.or(trim),
            };
            let message = match wake {
                Some(deadline) => {
                    match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    {
                        Ok(message) => Some(message),
                        Err(std_mpsc::RecvTimeoutError::Timeout) => None,
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match receiver.recv() {
                    Ok(message) => Some(message),
                    Err(_) => return,
                },
            };
            self.load.busy.store(true, Ordering::Relaxed);
            if let Some(message) = message {
                self.handle(message);
            }
            self.fire_due(Instant::now());
            self.trim();
            self.load.busy.store(false, Ordering::Relaxed);
        }
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Open {
                host,
                claimed,
                events,
                interrupted,
            } => {
                self.opened += 1;
                let engine = match self.spares.pop() {
                    Some(engine) => {
                        if !claimed {
                            self.load.spares.fetch_sub(1, Ordering::AcqRel);
                        }
                        *engine.interrupted.borrow_mut() = Arc::clone(&interrupted);
                        Some(engine)
                    }
                    None => match Engine::new(&interrupted) {
                        Ok(engine) => Some(engine),
                        Err(error) => {
                            tracing::error!(
                                target: "nanocode_code_mode",
                                %error,
                                "failed to start an embedded QuickJS runtime"
                            );
                            None
                        }
                    },
                };
                let events = if engine.is_some() {
                    events
                } else {
                    mpsc::unbounded_channel().0
                };
                self.hosts.insert(
                    host,
                    Host {
                        events,
                        interrupted,
                        engine,
                        execution: None,
                        deadline: None,
                    },
                );
            }
            Message::Command { host, command } => self.command(host, command),
            Message::Close { host, stopped } => {
                self.close(host);
                if let Some(stopped) = stopped {
                    let _ = stopped.send(());
                }
            }
        }
    }

    fn close(&mut self, id: u64) {
        let Some(mut host) = self.hosts.remove(&id) else {
            return;
        };
        self.load.live.fetch_sub(1, Ordering::Relaxed);
        if let Some(deadline) = host.deadline.take() {
            self.deadlines.remove(&(deadline, id));
        }
        let idle = host.execution.is_none();
        drop(host.execution.take());
        let engine = host.engine.take();
        drop(host);
        let Some(mut engine) = engine else {
            return;
        };
        if !idle || self.spares.len() >= self.demand.max(self.opened).max(1) {
            return;
        }
        *engine.interrupted.borrow_mut() = Arc::new(AtomicBool::new(false));
        if engine.prepared.is_none() {
            match Prepared::new(
                &engine.runtime,
                &self.idle_events,
                &engine.rejections,
                false,
            ) {
                Ok(prepared) => engine.prepared = Some(prepared),
                Err(_) => return,
            }
        }
        self.spares.push(engine);
        self.load.spares.fetch_add(1, Ordering::AcqRel);
    }

    fn trim(&mut self) {
        if self.window.elapsed() < SPARE_WINDOW {
            return;
        }
        self.demand = self.opened;
        self.opened = 0;
        let keep = self.demand.max(1);
        let dropped = self.spares.len().saturating_sub(keep);
        self.spares.truncate(keep);
        self.load.spares.fetch_sub(
            isize::try_from(dropped).unwrap_or(isize::MAX),
            Ordering::AcqRel,
        );
        self.window = Instant::now();
    }

    fn kill(&mut self, id: u64) {
        let Some(host) = self.hosts.get_mut(&id) else {
            return;
        };
        if let Some(deadline) = host.deadline.take() {
            self.deadlines.remove(&(deadline, id));
        }
        drop(host.execution.take());
        drop(host.engine.take());
        host.events = mpsc::unbounded_channel().0;
    }

    fn command(&mut self, id: u64, command: HostCommand) {
        let Some(host) = self.hosts.get_mut(&id) else {
            return;
        };
        let current = host
            .execution
            .as_ref()
            .map(|execution| execution.execution_id);
        let outcome = match command {
            HostCommand::Start(start) => {
                if current.is_some() {
                    return;
                }
                self.start(id, start)
            }
            HostCommand::ToolResult {
                execution_id,
                id: call,
                encoded,
                success,
            } if current == Some(execution_id) => {
                if let Some(stashed) = host
                    .execution
                    .as_mut()
                    .and_then(|execution| execution.held.as_mut())
                {
                    stashed.push((call, encoded, success));
                    return;
                }
                self.step(id, |ctx, state, promise| {
                    resolve_tool(ctx, state, call, encoded, success)?;
                    drain_jobs(ctx, state);
                    settle(ctx, state, promise)
                })
            }
            HostCommand::Hold { execution_id } if current == Some(execution_id) => {
                let Some(execution) = host.execution.as_mut() else {
                    return;
                };
                if execution.held.is_some() {
                    return;
                }
                let sent = execution
                    .prepared
                    .state
                    .borrow()
                    .send(RuntimeEvent::Held {
                        cell_id: execution_id,
                    })
                    .is_ok();
                if !sent {
                    self.kill(id);
                    return;
                }
                execution.held = Some(Vec::new());
                if let Some(deadline) = host.deadline.take() {
                    self.deadlines.remove(&(deadline, id));
                }
                return;
            }
            HostCommand::Release { execution_id } if current == Some(execution_id) => {
                let Some(stashed) = host
                    .execution
                    .as_mut()
                    .and_then(|execution| execution.held.take())
                else {
                    return;
                };
                self.step(id, |ctx, state, promise| {
                    for (call, encoded, success) in stashed {
                        resolve_tool(ctx, state, call, encoded, success)?;
                        drain_jobs(ctx, state);
                    }
                    settle(ctx, state, promise)
                })
            }
            HostCommand::ToolResult { .. }
            | HostCommand::Hold { .. }
            | HostCommand::Release { .. } => return,
        };
        self.after(id, outcome);
    }

    fn start(&mut self, id: u64, start: StartExecution) -> Result<Step, String> {
        let Some(host) = self.hosts.get_mut(&id) else {
            return Ok(Step::Finished);
        };
        let Some(engine) = host.engine.as_mut() else {
            return Err("embedded QuickJS code-mode host is unavailable".to_owned());
        };
        host.interrupted.store(false, Ordering::Release);
        if let Err(error) = install_prelude(&engine.runtime, start.prelude.as_deref()) {
            let _ = host.events.send(RuntimeEvent::Error {
                cell_id: start.execution_id,
                message: error,
                stored: HashMap::new(),
            });
            return Ok(Step::Finished);
        }
        let prepared = match engine.prepare(&host.events, start.prelude.is_some()) {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::error!(
                    target: "nanocode_code_mode",
                    %error,
                    "failed to create a fresh embedded QuickJS context"
                );
                return Err(error);
            }
        };
        prepared.state.borrow_mut().event_tx = host.events.clone();
        let execution_id = start.execution_id;
        let started = prepared.context.with(|ctx| {
            let state = &prepared.state;
            begin_execution(state, execution_id, start.stored, start.journaling);
            let run_cell = prepared
                .run_cell
                .clone()
                .restore(&ctx)
                .catch(&ctx)
                .map_err(|error| format!("failed to restore the QuickJS bootstrap: {error}"))?;
            let promise = start_execution(
                &ctx,
                state,
                &run_cell,
                start.source,
                &start.tools,
                start.prelude.is_some(),
            )?;
            if let Some(reason) = replay_inputs(&ctx, state, &promise)? {
                state
                    .borrow()
                    .send(RuntimeEvent::Diverged {
                        cell_id: execution_id,
                        reason,
                    })
                    .map_err(|_| "embedded QuickJS execution observer closed".to_owned())?;
                return Ok(None);
            }
            match settle(&ctx, state, &promise)? {
                Step::Finished => Ok(None),
                Step::Pending => Ok(Some(Persistent::save(&ctx, promise))),
            }
        });
        match started {
            Ok(Some(promise)) => {
                host.execution = Some(Execution {
                    execution_id,
                    promise,
                    prepared,
                    held: None,
                });
                Ok(Step::Pending)
            }
            Ok(None) => {
                drop(prepared);
                engine.runtime.run_gc();
                Ok(Step::Finished)
            }
            Err(error) => Err(error),
        }
    }

    fn step(
        &mut self,
        id: u64,
        run: impl for<'js> FnOnce(
            &Ctx<'js>,
            &Rc<RefCell<ExecutionState>>,
            &Promise<'js>,
        ) -> Result<Step, String>,
    ) -> Result<Step, String> {
        let Some(execution) = self
            .hosts
            .get_mut(&id)
            .and_then(|host| host.execution.as_ref())
        else {
            return Ok(Step::Finished);
        };
        execution.prepared.context.with(|ctx| {
            let promise =
                execution.promise.clone().restore(&ctx).map_err(|error| {
                    format!("failed to restore the QuickJS cell promise: {error}")
                })?;
            run(&ctx, &execution.prepared.state, &promise)
        })
    }

    fn after(&mut self, id: u64, outcome: Result<Step, String>) {
        match outcome {
            Ok(Step::Pending) => self.schedule(id),
            Ok(Step::Finished) => self.finish(id),
            Err(_) => self.kill(id),
        }
    }

    fn finish(&mut self, id: u64) {
        let Some(host) = self.hosts.get_mut(&id) else {
            return;
        };
        if let Some(deadline) = host.deadline.take() {
            self.deadlines.remove(&(deadline, id));
        }
        if let Some(execution) = host.execution.take() {
            let Execution {
                promise, prepared, ..
            } = execution;
            drop(promise);
            drop(prepared);
            if let Some(engine) = host.engine.as_ref() {
                engine.runtime.run_gc();
            }
        }
    }

    fn schedule(&mut self, id: u64) {
        let Some(host) = self.hosts.get_mut(&id) else {
            return;
        };
        let next = host
            .execution
            .as_ref()
            .filter(|execution| execution.held.is_none())
            .and_then(|execution| {
                next_timeout_wait(&execution.prepared.state).map(|wait| Instant::now() + wait)
            });
        if host.deadline == next {
            return;
        }
        if let Some(deadline) = host.deadline.take() {
            self.deadlines.remove(&(deadline, id));
        }
        if let Some(deadline) = next {
            self.deadlines.insert((deadline, id));
            host.deadline = Some(deadline);
        }
    }

    fn fire_due(&mut self, now: Instant) {
        while let Some(&(deadline, id)) = self.deadlines.first() {
            if deadline > now {
                return;
            }
            self.deadlines.pop_first();
            if let Some(host) = self.hosts.get_mut(&id) {
                host.deadline = None;
            }
            let outcome = self.step(id, |ctx, state, promise| {
                let now = Instant::now();
                while let Some(timeout) = next_due_timeout(state, now) {
                    invoke_timeout(ctx, state, timeout)?;
                    drain_jobs(ctx, state);
                    if let Step::Finished = settle(ctx, state, promise)? {
                        return Ok(Step::Finished);
                    }
                }
                Ok(Step::Pending)
            });
            self.after(id, outcome);
        }
    }
}

fn settle(
    ctx: &Ctx<'_>,
    state: &Rc<RefCell<ExecutionState>>,
    promise: &Promise<'_>,
) -> Result<Step, String> {
    match completed_terminal(ctx, promise)? {
        Some(terminal) => {
            send_terminal(state, terminal)?;
            Ok(Step::Finished)
        }
        None => Ok(Step::Pending),
    }
}
