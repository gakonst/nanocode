mod audio;
mod local;
mod output;
use crate::code_mode_spec as spec;

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};

#[cfg(not(target_family = "wasm"))]
type CallFuture<'a> = futures_util::future::BoxFuture<'a, CompletedNestedCall>;
#[cfg(target_family = "wasm")]
type CallFuture<'a> = futures_util::future::LocalBoxFuture<'a, CompletedNestedCall>;

#[cfg(not(target_family = "wasm"))]
fn spawn(future: impl std::future::Future<Output = ()> + Send + 'static) {
    drop(tokio::spawn(future));
}

#[cfg(target_family = "wasm")]
fn spawn(future: impl std::future::Future<Output = ()> + 'static) {
    nanocode_platform::spawn_local(future);
}
use serde::Deserialize;
use serde_json::{Value, value::RawValue};

use nanocode_platform::Instant;
use nanocode_sync::{Atom, Leased, Owned, Sequencer, Signal, Slot, Turn};
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tracing::{Instrument, info_span};

use super::{ToolContext, ToolOutputBody, ToolOutputContent};
use crate::embedded::OwnedToolContext;
pub use crate::embedded::{
    CodeModeCell, CodeModeExecution, CodeModeNotification, CodeModeObserver, CodeModeUpdate,
    NestedToolCall,
};
pub use crate::{CodeModeToolObservation, CodeModeToolset};
pub use local::LocalCells;
#[cfg(target_family = "wasm")]
pub use nanocode_code_mode_engine::{DEFAULT_INTERRUPT_CHECKS, set_interrupt_checks};
use nanocode_code_mode_engine::{EmbeddedHost as ExecutionWorker, RuntimeEvent};
use nanocode_tool_contracts::{ToolProcessTraceWire, ToolReceipt};
pub use spec::{exec_spec, wait_spec};

const INITIAL_YIELD: Duration = Duration::from_secs(10);
const DEFAULT_WAIT_YIELD: Duration = Duration::from_secs(10);
const OBSERVER_YIELD_GRACE: Duration = Duration::from_secs(1);
const MIN_YIELD_FOR_OBSERVER_GRACE: Duration = Duration::from_secs(10);
const MAX_JS_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;
const EXEC_PRAGMA_PREFIX: &str = "// @exec:";
const CELL_RUNNING: u8 = 0;
const CELL_TERMINATING: u8 = 1;
const CELL_COMPLETION_CLAIMED: u8 = 2;
const CELL_CLOSED: u8 = 3;

pub struct CodeModeRuntime {
    admission: Sequencer,
    admission_epoch: Arc<AtomicU64>,

    cells: Arc<Atom<CellRegistry>>,
    stored: Arc<Atom<HashMap<String, Arc<RawValue>>>>,
    host: Arc<Slot<EmbeddedHost>>,
    current_turn: Arc<AtomicU64>,
    preempt: watch::Sender<(u64, u64)>,
    preempt_consumed: Arc<AtomicU64>,
    prelude: Option<Arc<str>>,
}

#[derive(Clone)]
pub struct CodeModeControl {
    current_turn: Arc<AtomicU64>,
    preempt: watch::Sender<(u64, u64)>,
    preempt_consumed: Arc<AtomicU64>,
    admission: Sequencer,
    admission_epoch: Arc<AtomicU64>,

    cells: Arc<Atom<CellRegistry>>,
    host: Arc<Slot<EmbeddedHost>>,
}

pub struct CodeModeQuiescence {
    _admission: Turn,
}

struct EmbeddedHost {
    worker: ExecutionWorker,
}

fn prewarmed_host() -> Slot<EmbeddedHost> {
    let host = match spawn_host() {
        Ok(host) => Some(host),
        Err(error) => {
            tracing::warn!(
                target: "nanocode_code_mode",
                %error,
                "embedded QuickJS code mode prewarm failed; the first cell will retry"
            );
            None
        }
    };
    Slot::new(host)
}

fn spawn_host() -> Result<EmbeddedHost, String> {
    let started_at = Instant::now();
    let span = info_span!(
        target: "nanocode_code_mode",
        "code_mode.host_spawn",
        otel.kind = "internal",
        otel.status_code = tracing::field::Empty,
        status = tracing::field::Empty,
        duration_ns = tracing::field::Empty,
    );
    let result = span
        .in_scope(ExecutionWorker::spawn)
        .map(|worker| EmbeddedHost { worker });
    span.record(
        "status",
        if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
    );
    span.record(
        "otel.status_code",
        if result.is_ok() { "OK" } else { "ERROR" },
    );
    span.record(
        "duration_ns",
        u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
    );
    result
}

#[derive(Clone)]
struct CellRegistry {
    next_cell_id: u64,
    live_cells: HashMap<u64, Arc<LiveCell>>,
}

struct LiveCell {
    tools: Arc<dyn CodeModeToolset>,
    id: u64,
    origin_call_id: String,
    session_id: String,
    turn_id: AtomicU64,
    observation: Arc<Owned<CellObservationState>>,
    lifecycle: Arc<CellLifecycle>,
    terminate: Slot<oneshot::Sender<()>>,
    finished: Arc<Signal>,
    relay: Slot<CellRelay>,
}

struct CellRelay {
    stop: Arc<Notify>,
    done: Arc<Signal>,
}

struct Finished(Arc<Signal>);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.fire();
    }
}

struct CellObservationState {
    updates: mpsc::UnboundedReceiver<CellUpdate>,

    replay: VecDeque<CellUpdate>,
    buffered: ObservationBuffer,
    receipts: Vec<Arc<dyn ToolReceipt>>,
}

#[derive(Default)]
struct ObservationBuffer {
    content: Vec<ToolOutputContent>,
    nested_calls: Vec<ObservedNestedCall>,
    notifications: Vec<CodeModeNotification>,
}

struct CellLifecycle {
    phase: AtomicU8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CellError {
    Busy,
}

enum CellUpdate {
    NestedCallStarted {
        call_id: String,
        name: String,
        input: Arc<Value>,
        delivered: bool,
    },
    NestedCall(ObservedNestedCall),
    Notification {
        notification: CodeModeNotification,
        delivered: bool,
    },
    Content(ToolOutputContent),
    Yielded,
    Completed,
    Terminated,
    ScriptFailed {
        message: String,
    },
    HostFailed(String),
}

struct IgnoreCodeModeUpdates;

impl CodeModeObserver for IgnoreCodeModeUpdates {
    fn update(&mut self, _update: CodeModeUpdate<'_>) {}
}

struct CompletedNestedCall {
    id: u64,
    input: Arc<Value>,
    call: NestedToolCall,
    process_trace: Option<ToolProcessTraceWire>,
    receipts: Vec<Arc<dyn ToolReceipt>>,
}

struct ObservedNestedCall {
    id: u64,
    call: NestedToolCall,
    process_trace: Option<ToolProcessTraceWire>,
    delivered: bool,
    receipts: Vec<Arc<dyn ToolReceipt>>,
}

struct PendingCall {
    call_id: String,
    name: String,
    input: Arc<Value>,
    started: Instant,
    started_after_ns: u64,
}

struct PendingCallReceipts {
    calls: HashMap<u64, PendingCall>,
    updates: mpsc::UnboundedSender<CellUpdate>,
}

impl Drop for PendingCallReceipts {
    fn drop(&mut self) {
        let message = "Code Mode cell ended before the tool returned; execution outcome unknown";
        for (id, pending) in self.calls.drain() {
            let call = NestedToolCall {
                call_id: pending.call_id,
                name: pending.name,
                input: Arc::unwrap_or_clone(pending.input),
                output: ToolOutputBody::Text(message.to_owned()),
                structured_result: serde_json::json!({
                    "error": message, "code": "CODE_MODE_CALL_INTERRUPTED", "outcome": "unknown",
                }),
                success: false,
                started_after_ns: pending.started_after_ns,
                duration_ns: u64::try_from(pending.started.elapsed().as_nanos())
                    .unwrap_or(u64::MAX),
                metadata: None,
            };
            let _ = self
                .updates
                .send(CellUpdate::NestedCall(ObservedNestedCall {
                    id,
                    call,
                    process_trace: None,
                    delivered: false,
                    receipts: Vec::new(),
                }));
        }
    }
}

enum CellTerminal {
    Completed {
        stored: HashMap<String, Arc<RawValue>>,
    },
    ScriptFailed {
        message: String,
        stored: HashMap<String, Arc<RawValue>>,
    },
    Terminated,
}

struct HostFailure {
    message: String,
}

impl CodeModeRuntime {
    pub fn new_with_turn(current_turn: Arc<AtomicU64>) -> Self {
        Self {
            admission: Sequencer::new(),
            admission_epoch: Arc::new(AtomicU64::new(0)),

            cells: Arc::new(Atom::new(CellRegistry {
                next_cell_id: 1,
                live_cells: HashMap::new(),
            })),
            stored: Arc::default(),
            host: Arc::new(prewarmed_host()),
            current_turn,
            preempt: watch::channel((0, 0)).0,
            preempt_consumed: Arc::new(AtomicU64::new(0)),
            prelude: None,
        }
    }

    #[must_use]
    pub fn with_prelude(mut self, prelude: impl Into<Arc<str>>) -> Self {
        self.prelude = Some(prelude.into());
        self
    }

    pub fn control(&self) -> CodeModeControl {
        CodeModeControl {
            current_turn: Arc::clone(&self.current_turn),
            preempt: self.preempt.clone(),
            preempt_consumed: Arc::clone(&self.preempt_consumed),
            admission: self.admission.clone(),
            admission_epoch: Arc::clone(&self.admission_epoch),

            cells: Arc::clone(&self.cells),
            host: Arc::clone(&self.host),
        }
    }

    pub async fn execute(
        &self,
        source: &str,
        tools: Arc<dyn CodeModeToolset>,
        context: OwnedToolContext,
    ) -> CodeModeExecution {
        self.execute_with_updates(source, tools, context, &mut IgnoreCodeModeUpdates)
            .await
    }

    pub async fn execute_with_updates(
        &self,
        source: &str,
        tools: Arc<dyn CodeModeToolset>,
        context: OwnedToolContext,
        observer: &mut dyn CodeModeObserver,
    ) -> CodeModeExecution {
        let started_at = Instant::now();
        let span = info_span!(
            target: "nanocode_code_mode",
            "code_mode.cell",
            otel.kind = "internal",
            otel.status_code = tracing::field::Empty,
            session.id = context.session_id.as_str(),
            tool.call_id = context.call_id.as_str(),
            cell.id = tracing::field::Empty,
            source.bytes = source.len(),
            source.lines = source.lines().count(),
            output.max_tokens = tracing::field::Empty,
            nested.count = tracing::field::Empty,
            running = tracing::field::Empty,
            status = tracing::field::Empty,
            duration_ns = tracing::field::Empty,
        );
        let execution = self
            .execute_inner(source, tools, context, started_at, observer)
            .instrument(span.clone())
            .await;
        span.record("nested.count", execution.nested_calls.len());
        finish_cell_span(&span, &execution, started_at);
        execution
    }

    async fn execute_inner(
        &self,
        source: &str,
        tools: Arc<dyn CodeModeToolset>,
        context: OwnedToolContext,
        started_at: Instant,
        observer: &mut dyn CodeModeObserver,
    ) -> CodeModeExecution {
        let preempt = Preemption {
            signal: self.preempt.subscribe(),
            turn: self.current_turn.load(Ordering::Acquire),
            seen: self.preempt_consumed.load(Ordering::Acquire),
            consumed: Arc::clone(&self.preempt_consumed),
        };
        let admission_epoch = self.admission_epoch.load(Ordering::Acquire);
        let source = match parse_exec_source(source) {
            Ok(source) => source,
            Err(message) => return failed_execution(started_at, &message, Vec::new()),
        };
        let output_token_budget = source
            .max_output_tokens
            .unwrap_or(crate::contract::DEFAULT_TOOL_OUTPUT_TOKENS);
        tracing::Span::current().record("output.max_tokens", output_token_budget);
        let context = context.with_output_token_budget(output_token_budget);

        let admission = self.admission.enter().await;
        if self.admission_epoch.load(Ordering::Acquire) != admission_epoch {
            return observed_execution(
                tools.as_ref(),
                "Script terminated",
                true,
                started_at,
                Vec::new(),
                Some(output_token_budget),
                Vec::new(),
                Vec::new(),
            );
        }
        let stored = HashMap::clone(&self.stored.get());
        let cell = {
            let cell_id = self.cells.update(CellRegistry::allocate_cell_id);
            tracing::Span::current().record("cell.id", cell_id);
            let cell = Arc::new(LiveCell::spawn(
                cell_id,
                self.current_turn.load(Ordering::Acquire),
                source.code,
                tools,
                context,
                stored,
                Arc::clone(&self.stored),
                Arc::clone(&self.host),
                self.prelude.clone(),
            ));
            self.cells.update(|registry| {
                registry.live_cells.insert(cell_id, Arc::clone(&cell));
            });
            cell
        };
        drop(admission);
        let observation = Arc::clone(&cell.observation).acquire_owned().await;
        let yield_after = source
            .yield_time_ms
            .map_or(INITIAL_YIELD, Duration::from_millis);
        let yield_after = observer_yield_timeout(yield_after);
        let (mut execution, running) = observe_cell(
            &cell,
            observation,
            started_at,
            ObservationMode::YieldAfter(yield_after, preempt),
            Some(output_token_budget),
            Some(&cell.origin_call_id),
            observer,
        )
        .await;
        if running {
            cell.yielded();
        }
        execution.cell = Some(CodeModeCell {
            origin_call_id: cell.origin_call_id.clone(),
            running,
        });
        tracing::Span::current().record("running", running);
        if !running {
            self.remove_and_join(&cell).await;
        }
        execution
    }

    pub async fn wait(&self, input: &str, context: ToolContext<'_>) -> CodeModeExecution {
        self.wait_with_updates(input, context, &mut IgnoreCodeModeUpdates)
            .await
    }

    pub async fn wait_with_updates(
        &self,
        input: &str,
        context: ToolContext<'_>,
        observer: &mut dyn CodeModeObserver,
    ) -> CodeModeExecution {
        let started_at = Instant::now();
        let span = info_span!(
            target: "nanocode_code_mode",
            "code_mode.wait",
            otel.kind = "internal",
            otel.status_code = tracing::field::Empty,
            session.id = tracing::field::Empty,
            tool.call_id = tracing::field::Empty,
            cell.id = tracing::field::Empty,
            wait.yield_time_ms = tracing::field::Empty,
            wait.terminate = tracing::field::Empty,
            running = tracing::field::Empty,
            status = tracing::field::Empty,
            duration_ns = tracing::field::Empty,
        );
        let execution = self
            .wait_inner(input, context.call_id(), observer, started_at)
            .instrument(span.clone())
            .await;
        if let Some(cell) = &execution.cell {
            span.record("running", cell.running);
        }
        finish_cell_span(&span, &execution, started_at);
        execution
    }

    async fn wait_inner(
        &self,
        input: &str,
        call_id: &str,
        observer: &mut dyn CodeModeObserver,
        started_at: Instant,
    ) -> CodeModeExecution {
        let preempt = Preemption {
            signal: self.preempt.subscribe(),
            turn: self.current_turn.load(Ordering::Acquire),
            seen: self.preempt_consumed.load(Ordering::Acquire),
            consumed: Arc::clone(&self.preempt_consumed),
        };
        let arguments = match serde_json::from_str::<WaitArguments>(input) {
            Ok(arguments) => arguments,
            Err(error) => {
                return failed_execution(
                    started_at,
                    &format!("failed to parse wait arguments: {error}"),
                    Vec::new(),
                );
            }
        };
        let cell_id = match arguments.cell_id.parse::<u64>() {
            Ok(cell_id) => cell_id,
            Err(error) => {
                return failed_execution(
                    started_at,
                    &format!("invalid exec cell ID `{}`: {error}", arguments.cell_id),
                    Vec::new(),
                );
            }
        };
        let span = tracing::Span::current();
        span.record("cell.id", cell_id);
        span.record("wait.terminate", arguments.terminate);
        span.record("wait.yield_time_ms", arguments.yield_time_ms);
        let Some(cell) = self
            .cells
            .read(|registry| registry.live_cells.get(&cell_id).cloned())
        else {
            return failed_execution(
                started_at,
                &format!("exec cell {cell_id} not found"),
                Vec::new(),
            );
        };
        span.record("session.id", cell.session_id.as_str());
        span.record("tool.call_id", cell.origin_call_id.as_str());
        cell.turn_id
            .store(self.current_turn.load(Ordering::Acquire), Ordering::Release);
        let observation = match cell.begin_observation().await {
            Ok(observation) => observation,
            Err(CellError::Busy) => {
                return failed_execution(
                    started_at,
                    &format!("exec cell {cell_id} already has an active observer"),
                    Vec::new(),
                );
            }
        };
        cell.tools.cell_observed(&cell.origin_call_id, true);
        let output_token_budget = arguments.max_tokens;
        if arguments.terminate {
            cell.request_terminate();
            let (mut execution, running) = observe_cell(
                &cell,
                observation,
                started_at,
                ObservationMode::Terminate,
                output_token_budget,
                Some(call_id),
                observer,
            )
            .await;
            if !running {
                self.remove_and_join(&cell).await;
            }
            execution.cell = Some(CodeModeCell {
                origin_call_id: cell.origin_call_id.clone(),
                running,
            });
            return execution;
        }
        let yield_time = Duration::from_millis(arguments.yield_time_ms);
        let yield_time = observer_yield_timeout(yield_time);
        let (mut execution, running) = observe_cell(
            &cell,
            observation,
            started_at,
            ObservationMode::YieldAfter(yield_time, preempt),
            output_token_budget,
            Some(call_id),
            observer,
        )
        .await;
        if running {
            cell.yielded();
        } else {
            self.remove_and_join(&cell).await;
        }
        execution.cell = Some(CodeModeCell {
            origin_call_id: cell.origin_call_id.clone(),
            running,
        });
        execution
    }

    async fn remove_and_join(&self, cell: &Arc<LiveCell>) {
        self.cells.update(|registry| {
            if registry
                .live_cells
                .get(&cell.id)
                .is_some_and(|registered| Arc::ptr_eq(registered, cell))
            {
                registry.live_cells.remove(&cell.id);
            }
        });
        cell.join().await;
    }
}

fn finish_cell_span(span: &tracing::Span, execution: &CodeModeExecution, started_at: Instant) {
    span.record(
        "status",
        if execution.success {
            "completed"
        } else {
            "failed"
        },
    );
    span.record(
        "otel.status_code",
        if execution.success { "OK" } else { "ERROR" },
    );
    span.record(
        "duration_ns",
        u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
    );
}

fn observer_yield_timeout(yield_time: Duration) -> Duration {
    if yield_time >= MIN_YIELD_FOR_OBSERVER_GRACE {
        yield_time.saturating_add(OBSERVER_YIELD_GRACE)
    } else {
        yield_time
    }
}

impl CodeModeControl {
    pub fn preempt_turn(&self) {
        let turn = self.current_turn.load(Ordering::Acquire);
        self.preempt
            .send_modify(|generation| *generation = (turn, generation.1.wrapping_add(1)));
    }

    pub fn settle_preempt(&self) {
        self.preempt_consumed
            .store(self.preempt.borrow().1, Ordering::Release);
    }

    pub async fn detach_turn(
        &self,
        turn_id: u64,
        sink: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) {
        let cells = self.cells.read(|registry| {
            registry
                .live_cells
                .values()
                .filter(|cell| cell.turn_id.load(Ordering::Acquire) == turn_id)
                .cloned()
                .collect::<Vec<_>>()
        });
        for cell in cells {
            cell.detach(sink(&cell.origin_call_id));
        }
    }

    pub async fn terminate_turn(
        &self,
        turn_id: u64,
        mut observer: Option<&mut dyn CodeModeObserver>,
    ) {
        let _admission = self.admission.enter().await;
        self.admission_epoch.fetch_add(1, Ordering::AcqRel);
        let cells = self.cells.update(|registry| {
            let ids = registry
                .live_cells
                .iter()
                .filter_map(|(id, cell)| {
                    (cell.turn_id.load(Ordering::Acquire) == turn_id).then_some(*id)
                })
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| registry.live_cells.remove(&id))
                .collect::<Vec<_>>()
        });
        for cell in &cells {
            cell.request_terminate();
        }
        for cell in cells {
            if let Some(observer) = observer.as_mut() {
                cell.join_and_observe(*observer).await;
            } else {
                cell.join().await;
            }
        }
    }

    pub async fn terminate_all(
        &self,
        mut observer: Option<&mut dyn CodeModeObserver>,
    ) -> CodeModeQuiescence {
        let admission = self.admission.enter().await;
        self.admission_epoch.fetch_add(1, Ordering::AcqRel);
        let cells = self
            .cells
            .update(|registry| std::mem::take(&mut registry.live_cells))
            .into_values()
            .collect::<Vec<_>>();
        for cell in &cells {
            cell.request_terminate();
        }
        for cell in cells {
            if let Some(observer) = observer.as_mut() {
                cell.join_and_observe(*observer).await;
            } else {
                cell.join().await;
            }
        }

        if let Some(mut host) = self.host.take() {
            host.worker.terminate().await;
        }
        CodeModeQuiescence {
            _admission: admission,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecPragma {
    #[serde(default)]
    yield_time_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
}

struct ParsedExecSource {
    code: String,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<usize>,
}

fn parse_exec_source(input: &str) -> Result<ParsedExecSource, String> {
    if input.trim().is_empty() {
        return Err(
            "exec expects raw JavaScript source text (non-empty). Provide JS only, optionally with first-line `// @exec: {\"yield_time_ms\": 10000, \"max_output_tokens\": 1000}`."
                .to_owned(),
        );
    }
    let mut source = ParsedExecSource {
        code: input.to_owned(),
        yield_time_ms: None,
        max_output_tokens: None,
    };
    let mut lines = input.splitn(2, '\n');
    let first_line = lines.next().unwrap_or_default();
    let rest = lines.next().unwrap_or_default();
    let Some(pragma) = first_line.trim_start().strip_prefix(EXEC_PRAGMA_PREFIX) else {
        return Ok(source);
    };
    if rest.trim().is_empty() {
        return Err(
            "exec pragma must be followed by JavaScript source on subsequent lines".to_owned(),
        );
    }
    let directive = pragma.trim();
    if directive.is_empty() {
        return Err(
            "exec pragma must be a JSON object with supported fields `yield_time_ms` and `max_output_tokens`"
                .to_owned(),
        );
    }
    let value: Value = serde_json::from_str(directive).map_err(|error| {
        format!(
            "exec pragma must be valid JSON with supported fields `yield_time_ms` and `max_output_tokens`: {error}"
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        "exec pragma must be a JSON object with supported fields `yield_time_ms` and `max_output_tokens`"
            .to_owned()
    })?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "yield_time_ms" | "max_output_tokens"))
    {
        return Err(format!(
            "exec pragma only supports `yield_time_ms` and `max_output_tokens`; got `{key}`"
        ));
    }
    let pragma: ExecPragma = serde_json::from_value(value).map_err(|error| {
        format!(
            "exec pragma fields `yield_time_ms` and `max_output_tokens` must be non-negative safe integers: {error}"
        )
    })?;
    if pragma
        .yield_time_ms
        .is_some_and(|yield_time_ms| yield_time_ms > MAX_JS_SAFE_INTEGER)
    {
        return Err(
            "exec pragma field `yield_time_ms` must be a non-negative safe integer".to_owned(),
        );
    }
    if pragma.max_output_tokens.is_some_and(|max_output_tokens| {
        u64::try_from(max_output_tokens).map_or(true, |max_output_tokens| {
            max_output_tokens > MAX_JS_SAFE_INTEGER
        })
    }) {
        return Err(
            "exec pragma field `max_output_tokens` must be a non-negative safe integer".to_owned(),
        );
    }
    rest.clone_into(&mut source.code);
    source.yield_time_ms = pragma.yield_time_ms;
    source.max_output_tokens = pragma.max_output_tokens;
    Ok(source)
}

#[derive(Deserialize)]
struct WaitArguments {
    cell_id: String,
    #[serde(default = "default_wait_yield_time_ms")]
    yield_time_ms: u64,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    terminate: bool,
}

const fn default_wait_yield_time_ms() -> u64 {
    DEFAULT_WAIT_YIELD.as_millis() as u64
}

impl CellRegistry {
    const fn allocate_cell_id(&mut self) -> u64 {
        let cell_id = self.next_cell_id;
        self.next_cell_id = self.next_cell_id.saturating_add(1);
        cell_id
    }
}

impl LiveCell {
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        id: u64,
        turn_id: u64,
        source: String,
        tools: Arc<dyn CodeModeToolset>,
        context: OwnedToolContext,
        stored: HashMap<String, Arc<RawValue>>,
        shared_stored: Arc<Atom<HashMap<String, Arc<RawValue>>>>,
        host: Arc<Slot<EmbeddedHost>>,
        prelude: Option<Arc<str>>,
    ) -> Self {
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let (terminate, terminate_rx) = oneshot::channel();
        let lifecycle = Arc::new(CellLifecycle::new());
        let actor_span = info_span!(
            target: "nanocode_code_mode",
            "code_mode.cell_actor",
            otel.kind = "internal",
            otel.status_code = tracing::field::Empty,
            session.id = context.session_id.as_str(),
            tool.call_id = context.call_id.as_str(),
            cell.id = id,
            runtime.first_event_ns = tracing::field::Empty,
            runtime.event_count = tracing::field::Empty,
            host.reused = tracing::field::Empty,
            host.wait_ns = tracing::field::Empty,
            host.termination_ns = tracing::field::Empty,
            status = tracing::field::Empty,
            duration_ns = tracing::field::Empty,
        );
        let origin_call_id = context.call_id.clone();
        let session_id = context.session_id.clone();
        let finished = Arc::new(Signal::new());
        let guard = Finished(Arc::clone(&finished));
        spawn(
            run_cell_actor(
                host,
                id,
                source,
                prelude,
                Arc::clone(&tools),
                context,
                stored,
                shared_stored,
                updates_tx,
                terminate_rx,
                Arc::clone(&lifecycle),
            )
            .instrument(actor_span)
            .map(move |()| drop(guard)),
        );
        Self {
            tools,
            id,
            origin_call_id,
            session_id,
            turn_id: AtomicU64::new(turn_id),
            observation: Arc::new(Owned::new(CellObservationState {
                updates,
                replay: VecDeque::new(),
                buffered: ObservationBuffer::default(),
                receipts: Vec::new(),
            })),
            lifecycle,
            terminate: Slot::new(Some(terminate)),
            finished,
            relay: Slot::default(),
        }
    }

    fn detach(&self, sink: Box<dyn CodeModeObserver>) {
        if !self.relay.is_empty() {
            return;
        }
        let stop = Arc::new(Notify::new());
        let done = Arc::new(Signal::new());
        let finished = Finished(Arc::clone(&done));
        if let Some(previous) = self.relay.put(CellRelay {
            stop: Arc::clone(&stop),
            done,
        }) {
            self.relay.put(previous);
            return;
        }
        let Some(observation) = Arc::clone(&self.observation).try_acquire_owned() else {
            self.relay.take();
            return;
        };
        spawn(async move {
            let _finished = finished;
            relay_cell_updates(observation, stop, sink).await;
        });
    }

    async fn finish_relay(&self, stop: bool) {
        if let Some(relay) = self.relay.take() {
            if stop {
                relay.stop.notify_one();
            }
            relay.done.wait().await;
        }
    }

    async fn begin_observation(&self) -> Result<Leased<CellObservationState>, CellError> {
        loop {
            self.finish_relay(true).await;
            if let Some(observation) = Arc::clone(&self.observation).try_acquire_owned() {
                return Ok(observation);
            }
            if self.relay.is_empty() {
                return Err(CellError::Busy);
            }
        }
    }

    fn yielded(&self) {
        self.tools.cell_observed(&self.origin_call_id, false);
        if self.lifecycle.closed() {
            self.tools.cell_observed(&self.origin_call_id, true);
        }
    }

    fn request_terminate(&self) {
        if self.lifecycle.request_termination()
            && let Some(terminate) = self.terminate.take()
        {
            let _ = terminate.send(());
        }
    }

    async fn join_and_observe(&self, observer: &mut dyn CodeModeObserver) {
        self.join_actor().await;
        self.finish_relay(true).await;

        let observation = Arc::clone(&self.observation).acquire_owned().await;
        let _ = observe_cell(
            self,
            observation,
            Instant::now(),
            ObservationMode::Terminate,
            None,
            None,
            observer,
        )
        .await;
    }

    async fn join(&self) {
        self.join_actor().await;

        self.finish_relay(false).await;
    }

    async fn join_actor(&self) {
        self.finished.wait().await;
    }
}

impl Drop for LiveCell {
    fn drop(&mut self) {
        self.request_terminate();
    }
}

impl CellLifecycle {
    const fn new() -> Self {
        Self {
            phase: AtomicU8::new(CELL_RUNNING),
        }
    }

    fn request_termination(&self) -> bool {
        self.phase
            .compare_exchange(
                CELL_RUNNING,
                CELL_TERMINATING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn claim_completion(&self) -> bool {
        self.phase
            .compare_exchange(
                CELL_RUNNING,
                CELL_COMPLETION_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn close(&self) {
        self.phase.store(CELL_CLOSED, Ordering::Release);
    }

    fn closed(&self) -> bool {
        self.phase.load(Ordering::Acquire) == CELL_CLOSED
    }
}

struct Preemption {
    signal: watch::Receiver<(u64, u64)>,
    turn: u64,
    seen: u64,
    consumed: Arc<AtomicU64>,
}

impl Preemption {
    fn take(&self) -> bool {
        let (turn, generation) = *self.signal.borrow();
        if turn != self.turn || generation == self.seen {
            return false;
        }
        self.consumed.store(generation, Ordering::Release);
        true
    }
}

enum ObservationMode {
    YieldAfter(Duration, Preemption),
    Terminate,
}

async fn relay_cell_updates(
    mut observation: Leased<CellObservationState>,
    stop: Arc<Notify>,
    mut sink: Box<dyn CodeModeObserver>,
) {
    loop {
        let update = tokio::select! {
            biased;
            () = stop.notified() => break,
            update = observation.updates.recv() => update,
        };
        let Some(mut update) = update else {
            sink.completed();
            break;
        };
        let terminal = matches!(
            update,
            CellUpdate::Completed
                | CellUpdate::Terminated
                | CellUpdate::ScriptFailed { .. }
                | CellUpdate::HostFailed(_)
        );
        match &mut update {
            CellUpdate::NestedCallStarted {
                call_id,
                name,
                input,
                delivered,
            } => {
                sink.update(CodeModeUpdate::NestedCallStarted {
                    call_id,
                    name,
                    input,
                });
                *delivered = true;
            }
            CellUpdate::NestedCall(call) => {
                sink.update(CodeModeUpdate::NestedCallCompleted(&call.call));
                call.delivered = true;
            }
            CellUpdate::Notification {
                notification,
                delivered,
            } => {
                sink.update(CodeModeUpdate::Notification(notification));
                *delivered = true;
            }
            _ => {}
        }
        observation.replay.push_back(update);
        if terminal {
            sink.completed();
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn observe_cell(
    cell: &LiveCell,
    mut observation: Leased<CellObservationState>,
    started_at: Instant,
    mode: ObservationMode,
    max_output_tokens: Option<usize>,
    carrier: Option<&str>,
    observer: &mut dyn CodeModeObserver,
) -> (CodeModeExecution, bool) {
    let (execution, running) = observe_updates(
        cell,
        &mut observation,
        started_at,
        mode,
        max_output_tokens,
        observer,
    )
    .await;
    if !running && let Some(carrier) = carrier {
        for receipt in observation.receipts.drain(..) {
            receipt.carried_by(Some(carrier));
        }
    }
    (execution, running)
}

#[allow(clippy::too_many_lines)]
async fn observe_updates(
    cell: &LiveCell,
    observation: &mut Leased<CellObservationState>,
    started_at: Instant,
    mode: ObservationMode,
    max_output_tokens: Option<usize>,
    observer: &mut dyn CodeModeObserver,
) -> (CodeModeExecution, bool) {
    let tools = cell.tools.as_ref();
    let (yield_after, terminating, mut preempt) = match mode {
        ObservationMode::YieldAfter(yield_after, preempt) => {
            (Some(yield_after), false, Some(preempt))
        }
        ObservationMode::Terminate => (None, true, None),
    };
    let yield_deadline =
        yield_after.and_then(|yield_after| Instant::now().checked_add(yield_after));
    let mut yield_timer =
        yield_after.map(|yield_after| Box::pin(nanocode_platform::sleep(yield_after)));
    let mut preempt_remaining = None;
    loop {
        let yield_deadline_elapsed =
            yield_deadline.is_some_and(|yield_deadline| yield_deadline <= Instant::now());
        if preempt_remaining.is_none() && preempt.as_ref().is_some_and(Preemption::take) {
            preempt_remaining = Some(observation.updates.len() + observation.replay.len());
        }
        let preempted = preempt_remaining.is_some();
        if preempt_remaining == Some(0) {
            let buffered = std::mem::take(&mut observation.buffered);
            return running_observation(
                tools,
                cell.id,
                started_at,
                buffered.content,
                max_output_tokens,
                buffered.nested_calls,
                buffered.notifications,
            );
        }
        let update = if let Some(update) = observation.replay.pop_front() {
            Some(update)
        } else if preempted {
            observation.updates.recv().await
        } else {
            tokio::select! {
            biased;
            () = async {
                match preempt.as_mut() {
                    Some(preemption) => loop {
                        if preemption.signal.changed().await.is_err() { std::future::pending::<()>().await; }
                        if preemption.take() { break; }
                    },
                    None => std::future::pending().await,
                }
            }, if !preempted => {
                preempt_remaining = Some(observation.updates.len() + observation.replay.len());
                continue;
            }

            () = async {
                match yield_timer.as_mut() {
                    Some(timer) => timer.as_mut().await,
                    None => std::future::pending().await,
                }
            } => {
                let buffered = std::mem::take(&mut observation.buffered);
                return running_observation(
                    tools,
                    cell.id,
                    started_at,
                    buffered.content,
                    max_output_tokens,
                    buffered.nested_calls,
                    buffered.notifications,
                );
            }
            update = observation.updates.recv(), if !yield_deadline_elapsed => update,
            }
        };
        if let Some(remaining) = &mut preempt_remaining {
            *remaining = remaining.saturating_sub(1);
        }
        match update {
            Some(CellUpdate::NestedCallStarted {
                call_id,
                name,
                input,
                delivered,
            }) => {
                if !delivered {
                    observer.update(CodeModeUpdate::NestedCallStarted {
                        call_id: &call_id,
                        name: &name,
                        input: &input,
                    });
                }
            }
            Some(CellUpdate::NestedCall(mut call)) => {
                if !call.delivered {
                    observer.update(CodeModeUpdate::NestedCallCompleted(&call.call));
                }
                observation.receipts.append(&mut call.receipts);
                observation.buffered.nested_calls.push(call);
            }
            Some(CellUpdate::Notification {
                notification,
                delivered,
            }) => {
                if !delivered {
                    observer.update(CodeModeUpdate::Notification(&notification));
                }
                observation.buffered.notifications.push(notification);
            }
            Some(CellUpdate::Content(item)) => observation.buffered.content.push(item),
            Some(CellUpdate::Yielded) if terminating => {}
            Some(CellUpdate::Yielded) => {
                let buffered = std::mem::take(&mut observation.buffered);
                return running_observation(
                    tools,
                    cell.id,
                    started_at,
                    buffered.content,
                    max_output_tokens,
                    buffered.nested_calls,
                    buffered.notifications,
                );
            }
            Some(CellUpdate::Completed) => {
                let buffered = std::mem::take(&mut observation.buffered);
                return (
                    observed_execution(
                        tools,
                        "Script completed",
                        true,
                        started_at,
                        buffered.content,
                        max_output_tokens,
                        buffered.nested_calls,
                        buffered.notifications,
                    ),
                    false,
                );
            }
            Some(CellUpdate::Terminated) => {
                let buffered = std::mem::take(&mut observation.buffered);
                return (
                    observed_execution(
                        tools,
                        "Script terminated",
                        true,
                        started_at,
                        buffered.content,
                        max_output_tokens,
                        buffered.nested_calls,
                        buffered.notifications,
                    ),
                    false,
                );
            }
            Some(CellUpdate::ScriptFailed { message }) => {
                observation
                    .buffered
                    .content
                    .push(ToolOutputContent::InputText {
                        text: format!("Script error:\n{message}"),
                    });
                let buffered = std::mem::take(&mut observation.buffered);
                return (
                    observed_execution(
                        tools,
                        "Script failed",
                        false,
                        started_at,
                        buffered.content,
                        max_output_tokens,
                        buffered.nested_calls,
                        buffered.notifications,
                    ),
                    false,
                );
            }
            Some(CellUpdate::HostFailed(message)) => {
                observation
                    .buffered
                    .content
                    .push(ToolOutputContent::InputText { text: message });
                let buffered = std::mem::take(&mut observation.buffered);
                return (
                    observed_execution(
                        tools,
                        "Script failed",
                        false,
                        started_at,
                        buffered.content,
                        max_output_tokens,
                        buffered.nested_calls,
                        buffered.notifications,
                    ),
                    false,
                );
            }
            None => {
                observation
                    .buffered
                    .content
                    .push(ToolOutputContent::InputText {
                        text: "local code-mode cell ended before a result".to_owned(),
                    });
                let buffered = std::mem::take(&mut observation.buffered);
                return (
                    observed_execution(
                        tools,
                        "Script failed",
                        false,
                        started_at,
                        buffered.content,
                        max_output_tokens,
                        buffered.nested_calls,
                        buffered.notifications,
                    ),
                    false,
                );
            }
        }
    }
}

fn running_observation(
    tools: &dyn CodeModeToolset,
    cell_id: u64,
    started_at: Instant,
    content: Vec<ToolOutputContent>,
    max_output_tokens: Option<usize>,
    nested_calls: Vec<ObservedNestedCall>,
    notifications: Vec<CodeModeNotification>,
) -> (CodeModeExecution, bool) {
    (
        observed_execution(
            tools,
            &format!("Script running with cell ID {cell_id}"),
            true,
            started_at,
            content,
            max_output_tokens,
            nested_calls,
            notifications,
        ),
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn observed_execution(
    tools: &dyn CodeModeToolset,
    status: &str,
    success: bool,
    started_at: Instant,
    mut content: Vec<ToolOutputContent>,
    max_output_tokens: Option<usize>,
    nested_calls: Vec<ObservedNestedCall>,
    notifications: Vec<CodeModeNotification>,
) -> CodeModeExecution {
    let observations = nested_calls
        .iter()
        .map(|observed| CodeModeToolObservation {
            call: &observed.call,
            process_trace: observed.process_trace.as_ref(),
        })
        .collect::<Vec<_>>();
    tools.append_observation_content(&observations, &mut content);
    let content = output::truncate_content(content, max_output_tokens);
    CodeModeExecution {
        cell: None,
        output: with_status(status, started_at.elapsed().as_secs_f64(), content),
        success,
        nested_calls: ordered_calls(nested_calls),
        notifications,
    }
}

impl EmbeddedHost {
    async fn drive_cell(
        &mut self,
        cell_id: u64,
        parent_call_id: &str,
        tools: &dyn CodeModeToolset,
        context: &OwnedToolContext,
        updates: &mpsc::UnboundedSender<CellUpdate>,
        actor_started_at: Instant,
    ) -> Result<CellTerminal, HostFailure> {
        let mut pending_calls: FuturesUnordered<CallFuture<'_>> = FuturesUnordered::new();
        let mut pending_receipts = PendingCallReceipts {
            calls: HashMap::new(),
            updates: updates.clone(),
        };
        let mut event_count = 0_u64;
        loop {
            tokio::select! {
                biased;
                completed = pending_calls.next(), if !pending_calls.is_empty() => {
                    let Some(completed) = completed else {
                        continue;
                    };

                    self.send_completed_call(cell_id, completed, &mut pending_receipts)?;
                }
                event = self.worker.read_event() => {
                    let event = event.map_err(HostFailure::new)?;
                    event_count = event_count.saturating_add(1);
                    if event_count == 1 {
                        tracing::Span::current().record(
                            "runtime.first_event_ns",
                            u64::try_from(actor_started_at.elapsed().as_nanos())
                                .unwrap_or(u64::MAX),
                        );
                    }
                    let event_cell_id = event.cell_id();
                    if event_cell_id != cell_id {
                        return Err(HostFailure::new(format!(
                            "local code-mode host returned cell {event_cell_id} while executing cell {cell_id}"
                        )));
                    }
                    match event {
                        RuntimeEvent::ToolCall {
                            id, name, input, ..
                        } => {
                            let nested_call_id = format!("{}/code-{id}", context.call_id);
                            let started = Instant::now();
                            let input = Arc::new(input);
                            pending_receipts.calls.insert(id, PendingCall {
                                call_id: nested_call_id.clone(),
                                name: name.clone(),
                                input: Arc::clone(&input),
                                started,
                                started_after_ns: u64::try_from(started.duration_since(actor_started_at).as_nanos()).unwrap_or(u64::MAX),
                            });
                            let _ = updates.send(CellUpdate::NestedCallStarted {
                                call_id: nested_call_id,
                                name: name.clone(),
                                input: Arc::clone(&input),
                                delivered: false,
                            });
                            let nested_call = execute_nested_call(
                                tools,
                                id,
                                name,
                                input,
                                context,
                                actor_started_at,
                            );
                            pending_calls.push(Box::pin(nested_call));
                        }
                        RuntimeEvent::Notify { text, .. } => {
                            let _ = updates.send(CellUpdate::Notification {
                                notification: CodeModeNotification::new(parent_call_id, text),
                                delivered: false,
                            });
                        }
                        RuntimeEvent::Content { content, .. } => {
                            let _ = updates.send(CellUpdate::Content(content));
                        }
                        RuntimeEvent::Yielded { .. } => {
                            let _ = updates.send(CellUpdate::Yielded);
                        }
                        RuntimeEvent::Done {
                            stored,
                            ..
                        } => {
                            tracing::Span::current().record("runtime.event_count", event_count);
                            return Ok(CellTerminal::Completed { stored });
                        }
                        RuntimeEvent::Error {
                            message,
                            stored,
                            ..
                        } => {
                            tracing::Span::current().record("runtime.event_count", event_count);
                            return Ok(CellTerminal::ScriptFailed {
                                message,
                                stored,
                            });
                        }
                    }
                }
            }
        }
    }

    fn send_completed_call(
        &mut self,
        cell_id: u64,
        completed: CompletedNestedCall,
        pending_receipts: &mut PendingCallReceipts,
    ) -> Result<(), HostFailure> {
        let CompletedNestedCall {
            id,
            input,
            mut call,
            process_trace,
            receipts,
        } = completed;
        let success = call.success;

        pending_receipts.calls.remove(&id);
        call.input = Arc::unwrap_or_clone(input);
        let encoded = serde_json::to_string(&call.structured_result).map_err(|error| {
            HostFailure::new(format!("failed to encode Code Mode tool result: {error}"))
        })?;
        let _ = pending_receipts
            .updates
            .send(CellUpdate::NestedCall(ObservedNestedCall {
                id,
                call,
                process_trace,
                delivered: false,
                receipts,
            }));
        self.worker
            .send_tool_result(cell_id, id, encoded, success)
            .map_err(HostFailure::new)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_cell_actor(
    shared_host: Arc<Slot<EmbeddedHost>>,
    cell_id: u64,
    source: String,
    prelude: Option<Arc<str>>,
    tools: Arc<dyn CodeModeToolset>,
    context: OwnedToolContext,
    stored: HashMap<String, Arc<RawValue>>,
    shared_stored: Arc<Atom<HashMap<String, Arc<RawValue>>>>,
    updates: mpsc::UnboundedSender<CellUpdate>,
    mut terminate: oneshot::Receiver<()>,
    lifecycle: Arc<CellLifecycle>,
) {
    let started_at = Instant::now();
    let host_wait_started_at = Instant::now();
    let (mut host, reused) = {
        let taken = shared_host.take();
        let reused = taken.is_some();
        let host = match taken {
            Some(host) => host,
            None => match spawn_host() {
                Ok(host) => host,
                Err(message) => {
                    tracing::Span::current().record("status", "failed");
                    tracing::Span::current().record("otel.status_code", "ERROR");
                    record_elapsed("duration_ns", started_at);
                    let update = if lifecycle.claim_completion() {
                        CellUpdate::HostFailed(message)
                    } else {
                        CellUpdate::Terminated
                    };
                    let _ = updates.send(update);
                    lifecycle.close();
                    return;
                }
            },
        };
        (host, reused)
    };
    record_elapsed("host.wait_ns", host_wait_started_at);
    tracing::Span::current().record("host.reused", reused);
    let run = async {
        host.worker
            .start_cell(
                cell_id,
                &source,
                stored,
                tools.nested_tool_metadata(),
                prelude,
            )
            .map_err(HostFailure::new)?;
        host.drive_cell(
            cell_id,
            &context.call_id,
            tools.as_ref(),
            &context,
            &updates,
            started_at,
        )
        .await
    };
    let selected = tokio::select! {
        biased;
        _ = &mut terminate => {
            None
        }
        terminal = run => Some(terminal),
    };
    let terminal = match selected {
        Some(terminal) if lifecycle.claim_completion() => terminal,
        Some(_) | None => {
            let termination_started_at = Instant::now();
            host.worker.terminate().await;
            record_elapsed("host.termination_ns", termination_started_at);
            Ok(CellTerminal::Terminated)
        }
    };
    let (status, otel_status) = match &terminal {
        Ok(CellTerminal::Completed { .. }) => ("completed", "OK"),
        Ok(CellTerminal::Terminated) => ("cancelled", "ERROR"),
        Ok(CellTerminal::ScriptFailed { .. }) | Err(_) => ("failed", "ERROR"),
    };
    tracing::Span::current().record("status", status);
    tracing::Span::current().record("otel.status_code", otel_status);
    let terminated = matches!(&terminal, Ok(CellTerminal::Terminated));
    let host_healthy = matches!(
        &terminal,
        Ok(CellTerminal::Completed { .. } | CellTerminal::ScriptFailed { .. })
    );
    match terminal {
        Ok(CellTerminal::Completed { stored }) => {
            shared_stored.update(|shared| {
                shared.extend(
                    stored
                        .iter()
                        .map(|(key, value)| (key.clone(), Arc::clone(value))),
                )
            });
            let _ = updates.send(CellUpdate::Completed);
        }
        Ok(CellTerminal::ScriptFailed { message, stored }) => {
            shared_stored.update(|shared| {
                shared.extend(
                    stored
                        .iter()
                        .map(|(key, value)| (key.clone(), Arc::clone(value))),
                )
            });
            let _ = updates.send(CellUpdate::ScriptFailed { message });
        }
        Ok(CellTerminal::Terminated) => {
            let _ = updates.send(CellUpdate::Terminated);
        }
        Err(failure) => {
            let _ = updates.send(CellUpdate::HostFailed(failure.message));
        }
    }
    if host_healthy {
        if let Some(mut displaced) = shared_host.put(host) {
            displaced.worker.terminate().await;
        }
    } else if !terminated {
        let termination_started_at = Instant::now();
        host.worker.terminate().await;
        record_elapsed("host.termination_ns", termination_started_at);
    }
    lifecycle.close();
    tools.cell_observed(&context.call_id, true);
    record_elapsed("duration_ns", started_at);
}

fn record_elapsed(field: &'static str, started_at: Instant) {
    tracing::Span::current().record(
        field,
        u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
    );
}

impl HostFailure {
    const fn new(message: String) -> Self {
        Self { message }
    }
}

fn ordered_calls(mut calls: Vec<ObservedNestedCall>) -> Vec<NestedToolCall> {
    calls.sort_unstable_by_key(|call| call.id);
    calls.into_iter().map(|call| call.call).collect()
}

async fn execute_nested_call(
    tools: &dyn CodeModeToolset,
    id: u64,
    name: String,
    input: Arc<Value>,
    context: &OwnedToolContext,
    cell_started_at: Instant,
) -> CompletedNestedCall {
    let started_at = Instant::now();
    let started_after_ns =
        u64::try_from(started_at.duration_since(cell_started_at).as_nanos()).unwrap_or(u64::MAX);
    let call_id = format!("{}/code-{id}", context.call_id);
    let owned = context;
    let context = owned.as_context();
    let context = ToolContext::new(
        context.model(),
        context.session_id(),
        &call_id,
        &[],
        context.output_token_budget(),
    )
    .with_shared_history(&owned.history)
    .with_instruction_revision(context.instruction_revision())
    .with_host_context(context.host_context())
    .with_turn_id(context.turn_id());
    let mut execution = tools
        .execute_nested(&name, Value::clone(&input), context)
        .await;
    let duration_ns = u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let value = execution.take_structured_result();
    let process_trace = execution.process_trace().copied().map(Into::into);
    let receipts = execution.take_receipts();
    for receipt in &receipts {
        receipt.carried_by(None);
    }
    CompletedNestedCall {
        id,
        input,
        process_trace,
        receipts,
        call: NestedToolCall {
            call_id,
            name,
            input: Value::Null,
            output: execution.output,
            structured_result: value,
            success: execution.success,
            started_after_ns,
            duration_ns,
            metadata: execution.metadata,
        },
    }
}

fn failed_execution(
    started_at: Instant,
    message: &str,
    nested_calls: Vec<NestedToolCall>,
) -> CodeModeExecution {
    let wall_time = started_at.elapsed().as_secs_f64();
    CodeModeExecution {
        cell: None,
        output: ToolOutputBody::Text(format!(
            "Script failed\nWall time {wall_time:.1} seconds\nOutput:\n{message}"
        )),
        success: false,
        nested_calls,
        notifications: Vec::new(),
    }
}

fn with_status(
    status: &str,
    wall_time: f64,
    mut content: Vec<ToolOutputContent>,
) -> ToolOutputBody {
    let header = format!("{status}\nWall time {wall_time:.1} seconds\nOutput:\n");
    if content.is_empty() {
        return ToolOutputBody::Text(header);
    }
    content.insert(0, ToolOutputContent::InputText { text: header });
    ToolOutputBody::Content(content)
}
