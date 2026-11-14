mod audio;
mod local;
mod output;
mod replay;
use crate::code_mode_spec as spec;

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};

use nanocode_sync::{FutureExt, StreamExt};

#[cfg(not(target_family = "wasm"))]
type CallFuture<'a> = nanocode_sync::future::BoxFuture<'a, CompletedNestedCall>;
#[cfg(target_family = "wasm")]
type CallFuture<'a> = nanocode_sync::future::LocalBoxFuture<'a, CompletedNestedCall>;
#[cfg(not(target_family = "wasm"))]
type FlushFuture = nanocode_sync::future::BoxFuture<'static, ()>;
#[cfg(target_family = "wasm")]
type FlushFuture = nanocode_sync::future::LocalBoxFuture<'static, ()>;

fn request_flush(
    record: Option<&Arc<CellRecord>>,
    flushing: &mut Option<FlushFuture>,
    again: &mut bool,
) {
    let Some(record) = record.filter(|record| record.persistence.is_some()) else {
        return;
    };
    if flushing.is_some() {
        *again = true;
        return;
    }
    let record = Arc::clone(record);
    *flushing = Some(Box::pin(async move { record.flush().await }));
}

#[cfg(not(target_family = "wasm"))]
fn spawn(future: impl std::future::Future<Output = ()> + Send + 'static) {
    nanocode_platform::spawn(future);
}

#[cfg(target_family = "wasm")]
fn spawn(future: impl std::future::Future<Output = ()> + 'static) {
    nanocode_platform::spawn_local(future);
}
use serde::Deserialize;
use serde_json::{Value, value::RawValue};

use nanocode_platform::Instant;
use nanocode_sync::{Atom, InOrder, Leased, Owned, Sequencer, Signal, Slot, Turn};
use nanocode_sync::{Notify, mpsc, oneshot, watch};
use nanocode_trace::{Instrument, info_span};

use super::{ToolContext, ToolOutput, ToolOutputBody, ToolOutputContent};
use crate::embedded::OwnedToolContext;
pub use crate::embedded::{
    CodeModeCell, CodeModeExecution, CodeModeNotification, CodeModeObserver, CodeModeUpdate,
    NestedToolCall,
};
pub use crate::{CodeModeToolObservation, CodeModeToolset};
pub use local::LocalCells;
pub use nanocode_code_mode_engine::JournalEntry;
pub use nanocode_code_mode_engine::bytes;
use nanocode_code_mode_engine::{
    CellTools, EmbeddedHost as ExecutionWorker, Interrupt, RuntimeEvent,
};
#[cfg(target_family = "wasm")]
pub use nanocode_code_mode_engine::{DEFAULT_INTERRUPT_CHECKS, set_interrupt_checks};
pub use nanocode_code_mode_engine::{
    SnapshotStore, cell_memory, set_cell_memory, set_snapshot_store,
};
use nanocode_tool_contracts::{ToolProcessTraceWire, ToolReceipt};
pub use replay::{
    CELL_JOURNAL_VERSION, CellHold, CellJournal, CellJournalHeader, CellJournalSink,
    CellJournalState, CellJournalUpdate, CellJournals, JournaledCall, JournaledCallState,
    MovedCells,
};
use replay::{CellLife, CellRecord, CellStatus, Hold, Replay, ReplayPlan, Restored};
pub use spec::{
    exec_spec, exec_spec_with, exec_spec_yielding, source_with_options, source_yielding, wait_spec,
    wait_spec_yielding, wait_yielding,
};

const INITIAL_YIELD: Duration = spec::DEFAULT_YIELD;
const DEFAULT_WAIT_YIELD: Duration = spec::DEFAULT_YIELD;
const OBSERVER_YIELD_GRACE: Duration = Duration::from_secs(1);
const MIN_YIELD_FOR_OBSERVER_GRACE: Duration = Duration::from_secs(10);
use spec::EXEC_PRAGMA_PREFIX;
const CELL_RUNNING: u8 = 0;
const CELL_TERMINATING: u8 = 1;
const CELL_COMPLETION_CLAIMED: u8 = 2;
const CELL_CLOSED: u8 = 3;
const LOGGED_NESTED_CALLS_PER_TOOL: u64 = 100;
const CALL_LIMIT: &str = "call_limit";
const CALL_INTERRUPTED: &str = "CODE_MODE_CALL_INTERRUPTED";
const STREAM_OUTPUT_AFTER: Duration = Duration::from_millis(500);
const STREAMED_OUTPUT_BYTES: usize = 64 * 1024;
const TERMINATED_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct CellGuard {
    pub calls_per_tool: u64,
    pub refused_before_stop: usize,
    pub failing_streak: usize,
    pub failing_stall: Duration,
}

#[derive(Clone, Copy)]
struct Limits {
    nested: usize,
    guard: Option<CellGuard>,
}

#[derive(Clone, Copy)]
struct Yields {
    exec: Duration,
    wait: Duration,
}

impl Default for Yields {
    fn default() -> Self {
        Self {
            exec: INITIAL_YIELD,
            wait: DEFAULT_WAIT_YIELD,
        }
    }
}

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
    journals: Option<CellJournals>,
    limits: Limits,
    yields: Yields,
    catalog: Atom<Option<Arc<NestedCatalog>>>,
}

struct NestedCatalog {
    metadata: Arc<[Value]>,
    encoded: Arc<CellTools>,
}

impl NestedCatalog {
    fn encodes(&self, metadata: &Arc<[Value]>) -> bool {
        Arc::ptr_eq(&self.metadata, metadata) || *self.metadata == **metadata
    }
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

async fn spawn_host() -> Result<EmbeddedHost, String> {
    let started_at = nanocode_platform::now();
    let span = info_span!(
        target: "nanocode_code_mode",
        "code_mode.host_spawn",
        otel.kind = "internal",
        otel.status_code = nanocode_trace::field::Empty,
        status = nanocode_trace::field::Empty,
        duration_ns = nanocode_trace::field::Empty,
    );
    let result = ExecutionWorker::spawn_available()
        .instrument(span.clone())
        .await
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
    live_cells: BTreeMap<u64, Arc<LiveCell>>,
}

struct LiveCell {
    tools: Arc<dyn CodeModeToolset>,
    id: u64,
    origin_call_id: String,
    session_id: String,
    turn_id: AtomicU64,
    observation: Arc<Owned<CellObservationState>>,
    lifecycle: Arc<CellLifecycle>,
    interrupt: Interrupt,
    terminate: Slot<oneshot::Sender<()>>,
    finished: Arc<Signal>,
    relay: Slot<CellRelay>,
    record: Option<Arc<CellRecord>>,
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
    streamed: usize,
    streamed_bytes: usize,
    content_seen: u64,
    running: Vec<(String, String)>,
}

#[derive(Default)]
struct ObservationBuffer {
    content: Vec<ToolOutputContent>,
    nested_calls: Vec<ObservedNestedCall>,
    notifications: Vec<CodeModeNotification>,
    returned: u64,
    failed: u64,
    latest: Option<(String, String)>,
    abandoned: Vec<String>,
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
        logged: bool,
    },
    NestedCall(ObservedNestedCall),
    Notification {
        notification: CodeModeNotification,
        delivered: bool,
    },
    Unlogged {
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
    encoded: Option<String>,
}

struct ObservedNestedCall {
    id: u64,
    call: NestedToolCall,
    process_trace: Option<ToolProcessTraceWire>,
    delivered: bool,
    logged: bool,
    receipts: Vec<Arc<dyn ToolReceipt>>,
}

struct PendingCall {
    call_id: String,
    name: String,
    input: Arc<Value>,
    started: Instant,
    started_after_ns: u64,
    logged: bool,
}

struct UnloggedCalls {
    parent_call_id: String,
    counts: HashMap<String, u64>,
    updates: mpsc::UnboundedSender<CellUpdate>,
}

impl UnloggedCalls {
    fn admit(&mut self, name: &str) -> u64 {
        let count = self.counts.entry(name.to_owned()).or_default();
        *count += 1;
        *count
    }
}

struct CellWatch {
    guard: Option<CellGuard>,
    refused: usize,
    streak: Option<(String, String, usize)>,
    failed: BTreeMap<(String, String), usize>,
    succeeded: usize,
    noted: bool,
    stall: Option<Instant>,
}

enum Watched {
    Quiet,
    Note(String),
    Stop(String),
}

impl CellWatch {
    const fn new(guard: Option<CellGuard>) -> Self {
        Self {
            guard,
            refused: 0,
            streak: None,
            failed: BTreeMap::new(),
            succeeded: 0,
            noted: false,
            stall: None,
        }
    }

    fn settled(&mut self, call: &NestedToolCall, now: Instant) -> Watched {
        let Some(guard) = self.guard else {
            return Watched::Quiet;
        };
        let value = &call.structured_result;
        if call.success && value.get("isError") != Some(&Value::Bool(true)) {
            self.succeeded += 1;
            self.streak = None;
            if self.noted {
                self.stall = now.checked_add(guard.failing_stall);
            }
            return Watched::Quiet;
        }
        let code = value
            .get("code")
            .and_then(Value::as_str)
            .filter(|code| {
                !code.is_empty()
                    && code.len() <= 40
                    && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            })
            .unwrap_or("failed");
        *self
            .failed
            .entry((call.name.clone(), code.to_owned()))
            .or_default() += 1;
        if code == CALL_LIMIT {
            self.refused += 1;
            if self.refused >= guard.refused_before_stop {
                return Watched::Stop(format!(
                    "This cell called {} more than {} times and kept calling after {} calls were refused, so it was stopped. Batch the work into fewer calls or split it across cells.",
                    call.name, guard.calls_per_tool, guard.refused_before_stop
                ));
            }
        }
        let retryable = value.get("retryable").and_then(Value::as_bool) == Some(true);
        self.streak = match self.streak.take() {
            _ if retryable => None,
            Some((streak, _, n)) if streak == code => Some((streak, call.name.clone(), n + 1)),
            _ => Some((code.to_owned(), call.name.clone(), 1)),
        };
        let Some((code, tool, streak)) = &self.streak else {
            return Watched::Quiet;
        };
        if *streak < guard.failing_streak || self.noted {
            return Watched::Quiet;
        }
        self.noted = true;
        self.stall = now.checked_add(guard.failing_stall);
        let failed = self
            .failed
            .iter()
            .map(|((tool, code), count)| format!("{tool} {code} ×{count}"))
            .collect::<Vec<_>>()
            .join(", ");
        Watched::Note(format!(
            "This cell is still running, but its latest {streak} nested calls all failed with `{code}` (the last one {tool}), and calling again won't fix that. Its failed calls so far: {failed}. {} succeeded. Its later calls will most likely fail the same way. End it with `wait` and `terminate: true`, read the error, and change course: don't start the same calls again. If it runs on for {} seconds without a call succeeding, it is stopped.",
            self.succeeded,
            guard.failing_stall.as_secs()
        ))
    }

    fn stalled(&self) -> String {
        let stall = self.guard.map_or(0, |guard| guard.failing_stall.as_secs());
        format!(
            "This cell ran {stall} seconds after its nested calls kept failing without one succeeding, so it was stopped."
        )
    }
}

#[derive(Clone, Copy)]
enum Nested {
    Run,
    Recover,
    Refuse(u64),
}

impl Drop for UnloggedCalls {
    fn drop(&mut self) {
        let mut counts = self
            .counts
            .drain()
            .filter(|(_, count)| *count > LOGGED_NESTED_CALLS_PER_TOOL)
            .collect::<Vec<_>>();
        counts.sort();
        for (name, count) in counts {
            let _ = self.updates.send(CellUpdate::Unlogged {
                notification: CodeModeNotification::new(
                    &self.parent_call_id,
                    format!(
                        "{} more {name} calls in this cell ran without being logged one by one",
                        count - LOGGED_NESTED_CALLS_PER_TOOL
                    ),
                ),
                delivered: false,
            });
        }
    }
}

struct PendingCallReceipts {
    calls: BTreeMap<u64, PendingCall>,
    updates: mpsc::UnboundedSender<CellUpdate>,
}

impl Drop for PendingCallReceipts {
    fn drop(&mut self) {
        let message = "Code Mode cell ended before the tool returned; execution outcome unknown";
        for (id, pending) in std::mem::take(&mut self.calls) {
            let call = NestedToolCall {
                call_id: pending.call_id,
                name: pending.name,
                input: Arc::unwrap_or_clone(pending.input),
                output: ToolOutputBody::Text(message.to_owned()),
                structured_result: serde_json::json!({
                    "error": message, "code": CALL_INTERRUPTED, "outcome": "unknown",
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
                    logged: pending.logged,
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
    Terminated {
        stored: HashMap<String, Arc<RawValue>>,
    },
    Diverged {
        message: String,
    },
    Stopped {
        message: String,
    },
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
                live_cells: BTreeMap::new(),
            })),
            stored: Arc::default(),
            host: Arc::default(),
            current_turn,
            preempt: watch::channel((0, 0)).0,
            preempt_consumed: Arc::new(AtomicU64::new(0)),
            prelude: None,
            journals: None,
            limits: Limits {
                nested: usize::MAX,
                guard: None,
            },
            yields: Yields::default(),
            catalog: Atom::new(None),
        }
    }

    fn cell_tools(&self, metadata: &Arc<[Value]>) -> Result<Arc<CellTools>, String> {
        static LIVE: std::sync::LazyLock<Atom<Vec<std::sync::Weak<NestedCatalog>>>> =
            std::sync::LazyLock::new(Atom::default);
        let cached = self.catalog.read(|catalog| {
            catalog
                .as_ref()
                .filter(|catalog| catalog.encodes(metadata))
                .map(|catalog| Arc::clone(&catalog.encoded))
        });
        if let Some(encoded) = cached {
            return Ok(encoded);
        }
        let live = LIVE.read(|live| {
            live.iter()
                .filter_map(std::sync::Weak::upgrade)
                .find(|catalog| catalog.encodes(metadata))
        });
        let catalog = match live {
            Some(catalog) => catalog,
            None => {
                let catalog = Arc::new(NestedCatalog {
                    metadata: Arc::clone(metadata),
                    encoded: Arc::new(CellTools::encode(metadata)?),
                });
                let weak = Arc::downgrade(&catalog);
                LIVE.update(|live| {
                    live.retain(|catalog| catalog.strong_count() > 0);
                    live.push(std::sync::Weak::clone(&weak));
                });
                catalog
            }
        };
        let encoded = Arc::clone(&catalog.encoded);
        self.catalog.set(Some(catalog));
        Ok(encoded)
    }

    pub const fn set_nested_call_limit(&mut self, limit: usize) {
        self.limits.nested = limit;
    }

    pub const fn set_cell_guard(&mut self, guard: CellGuard) {
        self.limits.guard = Some(guard);
    }

    pub const fn set_default_yield(&mut self, after: Duration) {
        self.yields = Yields {
            exec: after,
            wait: after,
        };
    }

    #[must_use]
    pub fn with_journals(mut self, journals: CellJournals) -> Self {
        self.journals = Some(journals);
        self
    }

    pub fn set_journals(&mut self, journals: CellJournals) {
        self.journals = Some(journals);
    }

    #[must_use]
    pub const fn journals(&self) -> Option<&CellJournals> {
        self.journals.as_ref()
    }

    pub async fn adopt(&self, tools: Arc<dyn CodeModeToolset>, context: &OwnedToolContext) {
        let Some(journals) = &self.journals else {
            return;
        };
        journals.load(&context.session_id).await;
        self.restore_stored(journals, &context.session_id);
        if !journals.has_restored(&context.session_id) {
            return;
        }
        let _admission = self.admission.enter().await;
        let Some(restored) = journals.take_restored(&context.session_id).map(Arc::new) else {
            return;
        };
        let turn = self.current_turn.load(Ordering::Acquire);
        for journal in restored.cells.iter().cloned() {
            if journal.version != CELL_JOURNAL_VERSION || journal.session_id != context.session_id {
                nanocode_trace::warn!(
                    target: "nanocode_code_mode",
                    cell_id = journal.cell_id,
                    version = journal.version,
                    "skipped a Code Mode cell journal this runtime cannot replay"
                );
                continue;
            }
            if journal.finished {
                self.cells.update(|registry| {
                    registry.next_cell_id =
                        registry.next_cell_id.max(journal.cell_id.saturating_add(1));
                });
                journals.keep_dormant(journal, &restored);
                continue;
            }
            self.revive(&tools, context, journals, journal, &restored, turn);
        }
    }

    fn restore_stored(&self, journals: &CellJournals, session_id: &str) {
        let Some(restored) = journals.take_stored(session_id) else {
            return;
        };
        self.stored.update(|stored| {
            for (key, value) in &restored {
                stored
                    .entry(key.clone())
                    .or_insert_with(|| Arc::clone(value));
            }
        });
    }

    fn revive(
        &self,
        tools: &Arc<dyn CodeModeToolset>,
        context: &OwnedToolContext,
        journals: &CellJournals,
        journal: CellJournal,
        restored: &Restored,
        turn: u64,
    ) -> Option<Arc<LiveCell>> {
        let cell_id = journal.cell_id;
        let cell_context = OwnedToolContext {
            instruction_revision: context.instruction_revision,
            model: context.model.clone(),
            session_id: context.session_id.clone(),
            call_id: journal.origin_call_id.clone(),
            turn_id: context.turn_id.clone(),
            history: Arc::clone(&context.history),
            output_token_budget: journal
                .max_output_tokens
                .unwrap_or(context.output_token_budget),
            host_context: context.host_context.clone(),
        };
        let stored = journal.stored_values();
        let metadata: Arc<[Value]> = journal.tools.into();
        let catalog = match self.cell_tools(&metadata) {
            Ok(catalog) => catalog,
            Err(error) => {
                nanocode_trace::warn!(
                    target: "nanocode_code_mode",
                    cell_id,
                    %error,
                    "skipped a Code Mode cell journal whose tools cannot be encoded"
                );
                return None;
            }
        };
        let record = Arc::new(CellRecord {
            cell_id,
            session_id: journal.session_id.clone(),
            origin_call_id: journal.origin_call_id.clone(),
            script: journal.script.clone(),
            tools: metadata,
            stored: stored.clone(),
            started_at_ms: journal.started_at_ms,
            prelude: journal.prelude,
            yield_time_ms: journal.yield_time_ms,
            max_output_tokens: journal.max_output_tokens,
            journal: nanocode_code_mode_engine::SharedJournal::new(),
            status: nanocode_sync::Atom::new(CellStatus {
                observer_call_id: journal.observer_call_id.clone(),
                observer_yield_ms: journal.observer_yield_ms,
                observer_since_ms: journal.observer_since_ms,
                content_returned: journal.content_returned,
                ..CellStatus::new(CellLife::started(Replay::Replaying))
            }),
            hold: watch::channel(journals.holding(&journal.session_id)).0,
            persistence: journals.persistence(),
        });
        journals.register(&record);
        let plan = ReplayPlan {
            entries: journal.entries,
            calls: journal.calls,
            fallback: restored.fallback.clone(),
            outcomes: restored.outcomes.clone(),
            content_returned: journal.content_returned,
        };
        let cell = Arc::new(LiveCell::spawn(
            cell_id,
            turn,
            journal.script,
            Arc::clone(tools),
            catalog,
            cell_context,
            stored,
            Arc::clone(&self.stored),
            Arc::clone(&self.host),
            self.prelude.clone(),
            self.limits,
            Some((record, Some(plan))),
        ));
        self.cells.update(|registry| {
            registry.next_cell_id = registry.next_cell_id.max(cell_id.saturating_add(1));
            registry.live_cells.insert(cell_id, Arc::clone(&cell));
        });
        cell.yielded();
        Some(cell)
    }

    pub async fn recover_with_updates(
        &self,
        tools: Arc<dyn CodeModeToolset>,
        context: OwnedToolContext,
        observer: &mut dyn CodeModeObserver,
    ) -> Option<CodeModeExecution> {
        let started_at = nanocode_platform::now();
        let preempt = Preemption {
            signal: self.preempt.subscribe(),
            turn: self.current_turn.load(Ordering::Acquire),
            seen: self.preempt_consumed.load(Ordering::Acquire),
            consumed: Arc::clone(&self.preempt_consumed),
        };
        self.adopt(Arc::clone(&tools), &context).await;
        let call_id = context.call_id.as_str();
        let live = self.cells.read(|registry| {
            registry
                .live_cells
                .values()
                .find(|cell| {
                    cell.record.as_ref().is_some_and(|record| {
                        record.origin_call_id == call_id
                            || record.status(|status| {
                                status.observer_call_id.as_deref() == Some(call_id)
                            })
                    })
                })
                .cloned()
        });
        let cell = match live {
            Some(cell) => cell,
            None => {
                let journals = self.journals.as_ref()?;
                let (journal, restored) = journals.take_dormant(&context.session_id, call_id)?;
                let _admission = self.admission.enter().await;
                let turn = self.current_turn.load(Ordering::Acquire);
                self.revive(
                    &tools,
                    &context,
                    journals,
                    Arc::unwrap_or_clone(journal),
                    &restored,
                    turn,
                )?
            }
        };
        let record = cell.record.clone()?;
        let observation = cell.begin_observation().await.ok()?;
        let yield_time_ms = record.update(|status| {
            let observer_yield_ms = status
                .observer_yield_ms
                .filter(|_| status.observer_call_id.as_deref() == Some(call_id));
            let since = status
                .observer_since_ms
                .filter(|_| match status.observer_call_id.as_deref() {
                    Some(observer) => observer == call_id,
                    None => record.origin_call_id == call_id,
                })
                .unwrap_or_else(replay::wall_ms);
            status.observer_call_id = Some(call_id.to_owned());
            status.observer_yield_ms = observer_yield_ms;
            status.observer_since_ms = Some(since);
            observer_yield_ms
                .unwrap_or(record.yield_time_ms)
                .saturating_sub(replay::wall_ms().saturating_sub(since))
        });
        record.persist(Vec::new()).await;
        cell.turn_id
            .store(self.current_turn.load(Ordering::Acquire), Ordering::Release);
        cell.tools.cell_observed(&cell.origin_call_id, true);
        let yield_after = observer_yield_timeout(Duration::from_millis(yield_time_ms));
        let (mut execution, running) = observe_cell(
            &cell,
            observation,
            started_at,
            ObservationMode::YieldAfter(yield_after, preempt),
            record.max_output_tokens,
            Some(call_id),
            observer,
        )
        .await;
        if running {
            cell.yielded();
            if let Some(record) = &cell.record {
                record.persist(Vec::new()).await;
            }
        } else {
            self.remove_and_join(&cell).await;
        }
        execution.cell = Some(CodeModeCell {
            origin_call_id: cell.origin_call_id.clone(),
            running,
        });
        Some(execution)
    }

    #[must_use]
    pub fn with_prelude(mut self, prelude: impl Into<Arc<str>>) -> Self {
        self.prelude = Some(prelude.into());
        self
    }

    #[must_use]
    pub fn quiet(&self) -> bool {
        self.cells.read(|registry| registry.live_cells.is_empty())
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
        let started_at = nanocode_platform::now();
        let span = info_span!(
            target: "nanocode_code_mode",
            "code_mode.cell",
            otel.kind = "internal",
            otel.status_code = nanocode_trace::field::Empty,
            session.id = context.session_id.as_str(),
            tool.call_id = context.call_id.as_str(),
            cell.id = nanocode_trace::field::Empty,
            source.bytes = source.len(),
            source.lines = source.lines().count(),
            output.max_tokens = nanocode_trace::field::Empty,
            nested.count = nanocode_trace::field::Empty,
            running = nanocode_trace::field::Empty,
            status = nanocode_trace::field::Empty,
            duration_ns = nanocode_trace::field::Empty,
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
        nanocode_trace::Span::current().record("output.max_tokens", output_token_budget);
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
        if let Some(journals) = &self.journals {
            journals.load(&context.session_id).await;
            self.restore_stored(journals, &context.session_id);
            journals.discard_restored(&context.session_id).await;
            journals.release_finished(&context.session_id).await;
        }
        let stored = HashMap::clone(&self.stored.get());
        let metadata = tools.shared_nested_tool_metadata();
        let catalog = match self.cell_tools(&metadata) {
            Ok(catalog) => catalog,
            Err(message) => return failed_execution(started_at, &message, Vec::new()),
        };
        let cell = {
            let cell_id = self.cells.update(CellRegistry::allocate_cell_id);
            nanocode_trace::Span::current().record("cell.id", cell_id);
            let record = self.journals.as_ref().map(|journals| {
                let record = Arc::new(CellRecord {
                    cell_id,
                    session_id: context.session_id.clone(),
                    origin_call_id: context.call_id.clone(),
                    script: source.code.clone(),
                    tools: metadata,
                    stored: stored.clone(),
                    started_at_ms: Some(replay::wall_ms()),
                    prelude: self.prelude.is_some(),
                    yield_time_ms: source.yield_time_ms.unwrap_or(millis(self.yields.wait)),
                    max_output_tokens: Some(output_token_budget),
                    journal: nanocode_code_mode_engine::SharedJournal::new(),
                    status: nanocode_sync::Atom::new(CellStatus {
                        observer_since_ms: Some(replay::wall_ms()),
                        ..CellStatus::new(CellLife::started(Replay::Live))
                    }),
                    hold: watch::channel(journals.holding(&context.session_id)).0,
                    persistence: journals.persistence(),
                });
                journals.register(&record);
                record
            });
            if let Some(record) = &record {
                record.persist(Vec::new()).await;
            }
            let cell = Arc::new(LiveCell::spawn(
                cell_id,
                self.current_turn.load(Ordering::Acquire),
                source.code,
                tools,
                catalog,
                context,
                stored,
                Arc::clone(&self.stored),
                Arc::clone(&self.host),
                self.prelude.clone(),
                self.limits,
                record.map(|record| (record, None)),
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
            .map_or(self.yields.exec, Duration::from_millis);
        let yield_after = observer_yield_timeout(yield_after);
        let (mut execution, running) = observe_cell(
            &cell,
            observation,
            started_at,
            ObservationMode::Started(yield_after, preempt),
            Some(output_token_budget),
            Some(&cell.origin_call_id),
            observer,
        )
        .await;
        if running {
            cell.yielded();
            if let Some(record) = &cell.record {
                record.persist(Vec::new()).await;
            }
        }
        execution.cell = Some(CodeModeCell {
            origin_call_id: cell.origin_call_id.clone(),
            running,
        });
        nanocode_trace::Span::current().record("running", running);
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
        let started_at = nanocode_platform::now();
        let span = info_span!(
            target: "nanocode_code_mode",
            "code_mode.wait",
            otel.kind = "internal",
            otel.status_code = nanocode_trace::field::Empty,
            session.id = nanocode_trace::field::Empty,
            tool.call_id = nanocode_trace::field::Empty,
            cell.id = nanocode_trace::field::Empty,
            wait.yield_time_ms = nanocode_trace::field::Empty,
            wait.terminate = nanocode_trace::field::Empty,
            running = nanocode_trace::field::Empty,
            status = nanocode_trace::field::Empty,
            duration_ns = nanocode_trace::field::Empty,
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
        let span = nanocode_trace::Span::current();
        span.record("cell.id", cell_id);
        span.record("wait.terminate", arguments.terminate);
        let yield_time_ms = arguments
            .yield_time_ms
            .unwrap_or(millis(self.yields.wait));
        span.record("wait.yield_time_ms", yield_time_ms);
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
        if let Some(record) = &cell.record {
            record.update(|status| {
                status.observer_call_id = Some(call_id.to_owned());
                status.observer_yield_ms = Some(yield_time_ms);
                status.observer_since_ms = Some(replay::wall_ms());
            });
            record.persist(Vec::new()).await;
        }
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
        let yield_time = Duration::from_millis(yield_time_ms);
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
            if let Some(record) = &cell.record {
                record.persist(Vec::new()).await;
            }
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
        match (&self.journals, &cell.record) {
            (Some(journals), Some(record)) => journals.finished(record).await,
            _ => cell.retire(),
        }
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

fn finish_cell_span(span: &nanocode_trace::Span, execution: &CodeModeExecution, started_at: Instant) {
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
    #[must_use]
    pub fn quiet(&self) -> bool {
        self.cells.read(|registry| registry.live_cells.is_empty())
    }

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
            cell.retire();
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
            cell.retire();
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
    if let Some(key) = spec::json_wrapper_key(input) {
        return Err(format!(
            "exec expects raw JavaScript source text, but this input is a JSON object that wraps it (`{{\"{key}\": …}}`), so nothing ran. Send the JavaScript itself as the tool input. If this call's arguments are a JSON object, the source is the value of one JSON string in it and the arguments must parse: write every double quote of the source as `\\\"` and every backslash as `\\\\`, and end the object after that string."
        ));
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
    let field = |name: &str| {
        object
            .get(name)
            .filter(|value| !value.is_null())
            .map(|value| {
                spec::integer_value(value).map_err(|_| {
                    format!("exec pragma field `{name}` must be a non-negative safe integer, got {value}")
                })
            })
            .transpose()
    };
    source.yield_time_ms = field("yield_time_ms")?;
    source.max_output_tokens = field("max_output_tokens")?.map(token_budget);
    rest.clone_into(&mut source.code);
    Ok(source)
}

#[derive(Deserialize)]
struct WaitArguments {
    #[serde(deserialize_with = "cell_id")]
    cell_id: String,
    #[serde(default, deserialize_with = "requested_yield")]
    yield_time_ms: Option<u64>,
    #[serde(default, deserialize_with = "optional_integer")]
    max_tokens: Option<usize>,
    #[serde(default)]
    terminate: bool,
}

fn cell_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::String(text) => Ok(text),
        Value::Number(number) => Ok(number.to_string()),
        other => Err(serde::de::Error::custom(format!(
            "expected an exec cell ID, got {other}"
        ))),
    }
}

fn integer<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    spec::integer_value(&Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn optional_integer<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<usize>, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(None),
        value => spec::integer_value(&value)
            .map(|integer| Some(token_budget(integer)))
            .map_err(serde::de::Error::custom),
    }
}

fn token_budget(tokens: u64) -> usize {
    usize::try_from(tokens).unwrap_or(usize::MAX)
}

fn requested_yield<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    integer(deserializer).map(Some)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
        catalog: Arc<CellTools>,
        context: OwnedToolContext,
        stored: HashMap<String, Arc<RawValue>>,
        shared_stored: Arc<Atom<HashMap<String, Arc<RawValue>>>>,
        host: Arc<Slot<EmbeddedHost>>,
        prelude: Option<Arc<str>>,
        limits: Limits,
        journaling: Option<(Arc<CellRecord>, Option<ReplayPlan>)>,
    ) -> Self {
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let (terminate, terminate_rx) = oneshot::channel();
        let interrupt = Interrupt::new();
        let lifecycle = Arc::new(CellLifecycle::new());
        let actor_span = info_span!(
            target: "nanocode_code_mode",
            "code_mode.cell_actor",
            otel.kind = "internal",
            otel.status_code = nanocode_trace::field::Empty,
            session.id = context.session_id.as_str(),
            tool.call_id = context.call_id.as_str(),
            cell.id = id,
            runtime.first_event_ns = nanocode_trace::field::Empty,
            runtime.event_count = nanocode_trace::field::Empty,
            host.reused = nanocode_trace::field::Empty,
            host.wait_ns = nanocode_trace::field::Empty,
            host.termination_ns = nanocode_trace::field::Empty,
            status = nanocode_trace::field::Empty,
            duration_ns = nanocode_trace::field::Empty,
        );
        let origin_call_id = context.call_id.clone();
        let session_id = context.session_id.clone();
        let finished = Arc::new(Signal::new());
        let guard = Finished(Arc::clone(&finished));
        let content_seen = journaling
            .as_ref()
            .and_then(|(_, plan)| plan.as_ref())
            .map_or(0, |plan| plan.content_returned);
        let record = journaling.as_ref().map(|(record, _)| Arc::clone(record));
        let journaling = journaling.map(|(record, plan)| ActorJournaling {
            hold: record.hold.subscribe(),
            record,
            plan,
        });
        spawn(
            run_cell_actor(
                host,
                id,
                source,
                prelude,
                limits,
                Arc::clone(&tools),
                catalog,
                context,
                stored,
                shared_stored,
                updates_tx,
                terminate_rx,
                interrupt.clone(),
                Arc::clone(&lifecycle),
                journaling,
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
                streamed: 0,
                streamed_bytes: 0,
                content_seen,
                running: Vec::new(),
            })),
            lifecycle,
            interrupt,
            terminate: Slot::new(Some(terminate)),
            finished,
            relay: Slot::default(),
            record,
        }
    }

    fn retire(&self) {
        if let Some(record) = &self.record {
            record.update(|status| status.life = CellLife::Retired);
            record.forget();
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
            self.interrupt.raise();
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
            nanocode_platform::now(),
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
    Started(Duration, Preemption),
    YieldAfter(Duration, Preemption),
    Terminate,
}

async fn relay_cell_updates(
    mut observation: Leased<CellObservationState>,
    stop: Arc<Notify>,
    mut sink: Box<dyn CodeModeObserver>,
) {
    loop {
        let update = nanocode_sync::select! {
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
                logged,
            } => {
                if *logged {
                    sink.update(CodeModeUpdate::NestedCallStarted {
                        call_id,
                        name,
                        input,
                    });
                }
                *delivered = true;
            }
            CellUpdate::NestedCall(call) => {
                if call.logged {
                    sink.update(CodeModeUpdate::NestedCallCompleted(&call.call));
                }
                call.delivered = true;
            }
            CellUpdate::Notification {
                notification,
                delivered,
            }
            | CellUpdate::Unlogged {
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
    let (mut execution, running) = observe_updates(
        cell,
        &mut observation,
        started_at,
        mode,
        max_output_tokens,
        observer,
    )
    .await;
    for notice in cell.tools.session_notices(&cell.session_id) {
        execution
            .notifications
            .push(CodeModeNotification::new(&cell.origin_call_id, notice));
    }
    if let Some(record) = &cell.record {
        record.update(|status| status.content_returned = observation.content_seen);
    }
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
    let (yield_after, terminating, mut preempt, started) = match mode {
        ObservationMode::Started(yield_after, preempt) => {
            (Some(yield_after), false, Some(preempt), true)
        }
        ObservationMode::YieldAfter(yield_after, preempt) => {
            (Some(yield_after), false, Some(preempt), false)
        }
        ObservationMode::Terminate => (None, true, None, false),
    };
    let mut notice = (!terminating).then(|| tools.session_notice(&cell.session_id));
    if started
        && notice
            .as_mut()
            .is_some_and(|notice| notice.as_mut().now_or_never().is_some())
    {
        notice = None;
    }
    let yield_deadline =
        yield_after.and_then(|yield_after| nanocode_platform::now().checked_add(yield_after));
    let mut yield_timer =
        yield_after.map(|yield_after| Box::pin(nanocode_platform::sleep(yield_after)));
    let mut preempt_remaining = None;
    let mut stream_open = false;
    let mut stream_timer = Box::pin(nanocode_platform::sleep(STREAM_OUTPUT_AFTER));
    loop {
        let yield_deadline_elapsed =
            yield_deadline.is_some_and(|yield_deadline| yield_deadline <= nanocode_platform::now());
        if preempt_remaining.is_none()
            && (preempt.as_ref().is_some_and(Preemption::take) || yield_deadline_elapsed)
        {
            preempt_remaining = Some(observation.updates.len() + observation.replay.len());
        }
        let preempted = preempt_remaining.is_some();
        if preempt_remaining == Some(0) {
            return yielded_observation(tools, cell.id, started_at, observation, max_output_tokens);
        }
        let update = if let Some(update) = observation.replay.pop_front() {
            Some(update)
        } else if preempted {
            observation.updates.recv().await
        } else {
            nanocode_sync::select! {
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
                match notice.as_mut() {
                    Some(notice) => notice.as_mut().await,
                    None => std::future::pending().await,
                }
            }, if !preempted => {
                notice = None;
                preempt_remaining = Some(observation.updates.len() + observation.replay.len());
                continue;
            }

            () = async {
                match yield_timer.as_mut() {
                    Some(timer) => timer.as_mut().await,
                    None => std::future::pending().await,
                }
            } => {
                preempt_remaining = Some(observation.updates.len() + observation.replay.len());
                continue;
            }
            () = stream_timer.as_mut(), if !terminating && !stream_open => {
                stream_open = true;
                stream_output(&cell.origin_call_id, observation, observer);
                continue;
            }
            update = observation.updates.recv() => update,
            }
        };
        if let Some(remaining) = &mut preempt_remaining {
            *remaining = remaining.saturating_sub(1);
        }
        if stream_open
            && matches!(
                update,
                Some(
                    CellUpdate::NestedCallStarted { .. }
                        | CellUpdate::NestedCall(_)
                        | CellUpdate::Notification { .. }
                        | CellUpdate::Unlogged { .. }
                )
            )
        {
            stream_output(&cell.origin_call_id, observation, observer);
        }
        match update {
            Some(CellUpdate::NestedCallStarted {
                call_id,
                name,
                input,
                delivered,
                logged,
            }) => {
                observation.running.push((call_id.clone(), name.clone()));
                if logged && !delivered {
                    observer.update(CodeModeUpdate::NestedCallStarted {
                        call_id: &call_id,
                        name: &name,
                        input: &input,
                    });
                }
            }
            Some(CellUpdate::NestedCall(mut call)) => {
                observation
                    .running
                    .retain(|(call_id, _)| *call_id != call.call.call_id);
                if call
                    .call
                    .structured_result
                    .get("code")
                    .and_then(Value::as_str)
                    == Some(CALL_INTERRUPTED)
                {
                    observation.buffered.abandoned.push(call.call.name.clone());
                }
                observation.buffered.returned += 1;
                observation.buffered.failed += u64::from(!call.call.success);
                observation.buffered.latest = Some((
                    call.call.name.clone(),
                    output_tail(match &call.call.output {
                        ToolOutputBody::Text(text) => text.clone(),
                        ToolOutputBody::Content(content) => content_text(content),
                    }),
                ));
                if call.logged && !call.delivered {
                    observer.update(CodeModeUpdate::NestedCallCompleted(&call.call));
                }
                observation.receipts.append(&mut call.receipts);
                if call.logged {
                    observation.buffered.nested_calls.push(call);
                }
            }
            Some(CellUpdate::Unlogged {
                notification,
                delivered,
            }) => {
                if !delivered {
                    observer.update(CodeModeUpdate::Notification(&notification));
                }
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
            Some(CellUpdate::Content(item)) => {
                observation.content_seen += 1;
                let unstreamed = observation.buffered.content.len() > observation.streamed;
                match (observation.buffered.content.last_mut(), item) {
                    (
                        Some(ToolOutputContent::InputText { text: last }),
                        ToolOutputContent::InputText { text },
                    ) if unstreamed => {
                        last.push('\n');
                        last.push_str(&text);
                    }
                    (_, item) => observation.buffered.content.push(item),
                }
                if stream_open && observation.replay.is_empty() && observation.updates.is_empty() {
                    stream_output(&cell.origin_call_id, observation, observer);
                }
            }
            Some(CellUpdate::Yielded) if terminating => {}
            Some(CellUpdate::Yielded) => {
                return yielded_observation(
                    tools,
                    cell.id,
                    started_at,
                    observation,
                    max_output_tokens,
                );
            }
            Some(CellUpdate::Completed) => {
                let abandoned = abandoned_calls(
                    observation,
                    "the script ended without awaiting them",
                    "Await every call whose result or effect matters.",
                );
                if !abandoned.is_empty() {
                    observation
                        .buffered
                        .content
                        .push(ToolOutputContent::InputText {
                            text: abandoned.trim_start().to_owned(),
                        });
                }
                let buffered = take_buffered(observation);
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
                let abandoned = abandoned_calls(
                    observation,
                    "the script was terminated",
                    "Check what each did before calling it again.",
                );
                if !abandoned.is_empty() {
                    observation
                        .buffered
                        .content
                        .push(ToolOutputContent::InputText {
                            text: abandoned.trim_start().to_owned(),
                        });
                }
                if terminating {
                    let text = terminated_output(&observation.buffered.content);
                    stream_output(&cell.origin_call_id, observation, observer);
                    if !text.is_empty() {
                        observer.cell_terminated(&cell.origin_call_id, &text);
                    }
                }
                let buffered = take_buffered(observation);
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
                let abandoned = abandoned_calls(
                    observation,
                    "the script failed",
                    "A rejected call fails the script as soon as it is awaited, also through Promise.all; use Promise.allSettled, or try/catch around each call, so the other calls finish.",
                );
                observation
                    .buffered
                    .content
                    .push(ToolOutputContent::InputText {
                        text: format!("Script error:\n{message}{abandoned}"),
                    });
                let buffered = take_buffered(observation);
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
                let buffered = take_buffered(observation);
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
                let buffered = take_buffered(observation);
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

fn take_buffered(observation: &mut CellObservationState) -> ObservationBuffer {
    observation.streamed = 0;
    observation.streamed_bytes = 0;
    std::mem::take(&mut observation.buffered)
}

fn content_text(content: &[ToolOutputContent]) -> String {
    content
        .iter()
        .filter_map(|item| match item {
            ToolOutputContent::InputText { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const fn char_floor(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn terminated_output(content: &[ToolOutputContent]) -> String {
    output_tail(content_text(content))
}

fn output_tail(text: String) -> String {
    if text.len() <= TERMINATED_OUTPUT_BYTES {
        return text;
    }
    let mut start = text.len() - TERMINATED_OUTPUT_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…\n{}", &text[start..])
}

fn stream_output(
    call_id: &str,
    observation: &mut CellObservationState,
    observer: &mut dyn CodeModeObserver,
) {
    let start = observation.streamed.min(observation.buffered.content.len());
    observation.streamed = observation.buffered.content.len();
    if observation.streamed_bytes >= STREAMED_OUTPUT_BYTES {
        return;
    }
    let mut text = content_text(&observation.buffered.content[start..]);
    if text.is_empty() {
        return;
    }
    let room = STREAMED_OUTPUT_BYTES - observation.streamed_bytes;
    if text.len() > room {
        text.truncate(char_floor(&text, room));
        text.push_str("\n… the rest of the output arrives with the cell's result");
        observation.streamed_bytes = STREAMED_OUTPUT_BYTES;
    } else {
        observation.streamed_bytes += text.len();
    }
    observer.update(CodeModeUpdate::Notification(&CodeModeNotification::new(
        call_id, text,
    )));
}

fn abandoned_calls(observation: &mut CellObservationState, when: &str, advice: &str) -> String {
    let mut names = BTreeMap::<String, usize>::new();
    for name in std::mem::take(&mut observation.buffered.abandoned)
        .into_iter()
        .chain(
            std::mem::take(&mut observation.running)
                .into_iter()
                .map(|(_, name)| name),
        )
    {
        *names.entry(name).or_default() += 1;
    }
    if names.is_empty() {
        return String::new();
    }
    let names = names
        .iter()
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n\nNested calls that had not returned when {when} were abandoned: {names}. Whether each took effect is unknown. {advice}"
    )
}

fn yielded_observation(
    tools: &dyn CodeModeToolset,
    cell_id: u64,
    started_at: Instant,
    observation: &mut CellObservationState,
    max_output_tokens: Option<usize>,
) -> (CodeModeExecution, bool) {
    let running = std::mem::take(&mut observation.running);
    let mut buffered = take_buffered(observation);
    if buffered.content.is_empty() && (buffered.returned > 0 || !running.is_empty()) {
        let mut text = "The script printed nothing since the last yield.".to_owned();
        if let Some((name, tail)) = &buffered.latest {
            text.push_str(&format!(
                " {} nested call(s) returned since then, {} of them failed. The latest, {name}, returned:\n{tail}",
                buffered.returned, buffered.failed
            ));
        }
        if !running.is_empty() {
            let mut names = BTreeMap::<&str, usize>::new();
            for (_, name) in &running {
                *names.entry(name.as_str()).or_default() += 1;
            }
            let names = names
                .iter()
                .map(|(name, count)| format!("{count} {name}"))
                .collect::<Vec<_>>()
                .join(", ");
            text.push_str(&format!("\nNested calls still running: {names}."));
        }
        buffered.content.push(ToolOutputContent::InputText { text });
    }
    observation.running = running;
    running_observation(
        tools,
        cell_id,
        started_at,
        buffered.content,
        max_output_tokens,
        buffered.nested_calls,
        buffered.notifications,
    )
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

struct CellCalls<'a, 'b> {
    pending_calls: &'b mut InOrder<CallFuture<'a>>,
    pending_receipts: &'b mut PendingCallReceipts,
    unlogged: &'b mut UnloggedCalls,
    deferred: &'b mut std::collections::VecDeque<(u64, String, Box<str>, bool)>,
    limits: Limits,
    updates: &'b mpsc::UnboundedSender<CellUpdate>,
    record: Option<&'b CellRecord>,
    journal: Option<Arc<CellRecord>>,
    tools: &'a dyn CodeModeToolset,
    context: &'a OwnedToolContext,
    actor_started_at: Instant,
}

impl CellCalls<'_, '_> {
    fn dispatch(&mut self, id: u64, name: String, input: Value, host: bool) {
        if self.pending_calls.len() >= self.limits.nested {
            let text = input.to_string().into_boxed_str();
            if self.record.is_some() {
                self.queue(id, name.clone(), input, host);
            }
            self.deferred.push_back((id, name, text, host));
            return;
        }
        self.start(id, name, input, host, false);
    }

    fn resume_deferred(&mut self) {
        while self.pending_calls.len() < self.limits.nested
            && let Some((id, name, text, host)) = self.deferred.pop_front()
        {
            let input = serde_json::from_str(&text).expect("a JSON value's text parses");
            self.start(id, name, input, host, false);
        }
    }

    fn recover(&mut self, call: &JournaledCall) {
        self.start(
            call.id,
            call.name.clone(),
            call.input.clone(),
            call.host,
            true,
        );
    }

    fn start(&mut self, id: u64, name: String, input: Value, host: bool, recovering: bool) {
        let nested_call_id = format!("{}/code-{id}", self.context.call_id);
        let persists = self
            .journal
            .as_ref()
            .is_some_and(|record| record.persistence.is_some());
        let mut journaled = (self.record.is_some() || persists).then(|| JournaledCall {
            id,
            call_id: nested_call_id.clone(),
            name: name.clone(),
            input: input.clone(),
            host,
            state: JournaledCallState::Running,
        });
        if let Some(record) = self.record
            && let Some(call) = journaled.take()
        {
            let shared = Arc::new(call);
            record.update(|status| status.calls.insert(id, Arc::clone(&shared)));
            journaled = persists.then(|| JournaledCall::clone(&shared));
        }
        let started = nanocode_platform::now();
        let input = Arc::new(input);
        let admitted = self.unlogged.admit(&name);
        let logged = admitted <= LOGGED_NESTED_CALLS_PER_TOOL;
        let how = match self.limits.guard {
            _ if recovering => Nested::Recover,
            Some(guard) if admitted > guard.calls_per_tool => Nested::Refuse(guard.calls_per_tool),
            _ => Nested::Run,
        };
        self.pending_receipts.calls.insert(
            id,
            PendingCall {
                call_id: nested_call_id.clone(),
                name: name.clone(),
                input: Arc::clone(&input),
                started,
                started_after_ns: u64::try_from(
                    started.duration_since(self.actor_started_at).as_nanos(),
                )
                .unwrap_or(u64::MAX),
                logged,
            },
        );
        let _ = self.updates.send(CellUpdate::NestedCallStarted {
            call_id: nested_call_id,
            name: name.clone(),
            input: Arc::clone(&input),
            delivered: false,
            logged,
        });
        self.pending_calls.push(Box::pin(journaled_call(
            self.journal.clone(),
            journaled,
            execute_nested_call(
                self.tools,
                id,
                name,
                host,
                input,
                self.context,
                self.actor_started_at,
                how,
            ),
        )));
    }

    fn queue(&self, id: u64, name: String, input: Value, host: bool) {
        if let Some(record) = self.record {
            let queued = Arc::new(JournaledCall {
                id,
                call_id: format!("{}/code-{id}", self.context.call_id),
                name,
                input,
                host,
                state: JournaledCallState::Queued,
            });
            record.update(|status| status.calls.insert(id, Arc::clone(&queued)));
            record.journal(id);
        }
    }
}

impl EmbeddedHost {
    fn hold_cell(&mut self, cell_id: u64, record: Option<&CellRecord>) -> bool {
        let Some(record) = record else {
            return false;
        };
        if self.worker.hold(cell_id).is_ok() {
            true
        } else {
            record.update(|status| status.unsupported = true);
            false
        }
    }

    fn release_cell(
        &mut self,
        cell_id: u64,
        record: &CellRecord,
        calls: &mut CellCalls<'_, '_>,
    ) -> Result<(), HostFailure> {
        let undelivered = record.update(|status| {
            status.life = status.life.holding(Hold::Running);
            let (running, undelivered) = std::mem::take(&mut status.calls)
                .into_iter()
                .partition::<nanocode_sync::OrdMap<_, _>, _>(
                |(_, call)| matches!(call.state, JournaledCallState::Running),
            );
            status.calls = running;
            undelivered
        });
        for call in undelivered
            .into_iter()
            .map(|(_, call)| Arc::unwrap_or_clone(call))
        {
            match call.state {
                JournaledCallState::Completed { encoded, success } => self
                    .worker
                    .send_tool_result(cell_id, call.id, encoded, success)
                    .map_err(HostFailure::new)?,
                JournaledCallState::Queued => {
                    calls.dispatch(call.id, call.name, call.input, call.host);
                }
                JournaledCallState::Running => {}
            }
        }
        self.worker.release(cell_id).map_err(HostFailure::new)
    }

    fn resume_replayed(
        &mut self,
        cell_id: u64,
        plan: &ReplayPlan,
        held: bool,
        calls: &mut CellCalls<'_, '_>,
    ) -> Result<(), HostFailure> {
        for call in &plan.calls {
            if matches!(call.state, JournaledCallState::Queued) {
                if held {
                    calls.queue(call.id, call.name.clone(), call.input.clone(), call.host);
                } else {
                    calls.dispatch(call.id, call.name.clone(), call.input.clone(), call.host);
                }
                continue;
            }
            if !held
                && matches!(call.state, JournaledCallState::Running)
                && !plan.outcomes.contains_key(&call.call_id)
            {
                calls.recover(call);
                continue;
            }
            let (encoded, success) = plan.outcome(call);
            if held {
                if let Some(record) = calls.record {
                    let mut journaled = call.clone();
                    journaled.state = JournaledCallState::Completed { encoded, success };
                    let journaled = Arc::new(journaled);
                    record.update(|status| status.calls.insert(call.id, Arc::clone(&journaled)));
                    record.journal(call.id);
                }
            } else {
                self.worker
                    .send_tool_result(cell_id, call.id, encoded, success)
                    .map_err(HostFailure::new)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn drive_cell(
        &mut self,
        cell_id: u64,
        parent_call_id: &str,
        tools: &dyn CodeModeToolset,
        context: &OwnedToolContext,
        updates: &mpsc::UnboundedSender<CellUpdate>,
        actor_started_at: Instant,
        record: Option<Arc<CellRecord>>,
        mut plan: Option<ReplayPlan>,
        mut hold: Option<watch::Receiver<bool>>,
        limits: Limits,
    ) -> Result<CellTerminal, HostFailure> {
        let mut pending_calls: InOrder<CallFuture<'_>> = InOrder::new();
        let mut deferred = std::collections::VecDeque::new();
        let mut unlogged = UnloggedCalls {
            parent_call_id: parent_call_id.to_owned(),
            counts: HashMap::new(),
            updates: updates.clone(),
        };
        let mut pending_receipts = PendingCallReceipts {
            calls: BTreeMap::new(),
            updates: updates.clone(),
        };
        let mut watch = CellWatch::new(limits.guard);
        let mut event_count = 0_u64;
        let mut replaying = plan.is_some();
        let mut suppressed = plan.as_ref().map_or(0, |plan| plan.content_returned);
        let mut held = false;
        let mut flushing: Option<FlushFuture> = None;
        let mut flush_again = false;
        let journal = record.clone();
        if let Some(receiver) = hold.as_mut()
            && *receiver.borrow_and_update()
        {
            held = self.hold_cell(cell_id, record.as_deref());
        }
        loop {
            nanocode_sync::select! {
                changed = async {
                    match hold.as_mut() {
                        Some(receiver) => receiver.changed().await.is_ok(),
                        None => std::future::pending().await,
                    }
                } => {
                    let Some(receiver) = hold.as_mut() else {
                        continue;
                    };
                    if !changed {
                        hold = None;
                        continue;
                    }
                    let wanted = *receiver.borrow_and_update();
                    if wanted && !held {
                        held = self.hold_cell(cell_id, record.as_deref());
                    } else if !wanted && held && !replaying {
                        held = false;
                        if let Some(record) = record.as_deref() {
                            deferred.clear();
                            let mut calls = CellCalls {
                                pending_calls: &mut pending_calls,
                                pending_receipts: &mut pending_receipts,
                                unlogged: &mut unlogged,
                                deferred: &mut deferred,
                                limits,
                                updates,
                                record: Some(record),
                                journal: journal.clone(),
                                tools,
                                context,
                                actor_started_at,
                            };
                            self.release_cell(cell_id, record, &mut calls)?;
                        }
                    }
                }
                completed = pending_calls.next(), if !pending_calls.is_empty() => {
                    let Some(completed) = completed else {
                        continue;
                    };
                    match watch.settled(&completed.call, nanocode_platform::now()) {
                        Watched::Note(text) if !replaying => {
                            let _ = updates.send(CellUpdate::Notification {
                                notification: CodeModeNotification::new(parent_call_id, text),
                                delivered: false,
                            });
                            let _ = updates.send(CellUpdate::Yielded);
                        }
                        Watched::Stop(message) if !held && !replaying => {
                            return Ok(CellTerminal::Stopped { message });
                        }
                        Watched::Quiet | Watched::Note(_) | Watched::Stop(_) => {}
                    }
                    if self.send_completed_call(
                        cell_id,
                        completed,
                        &mut pending_receipts,
                        record.as_deref(),
                        held,
                    )? {
                        request_flush(record.as_ref(), &mut flushing, &mut flush_again);
                    }
                    if !held && !deferred.is_empty() {
                        let mut calls = CellCalls {
                            pending_calls: &mut pending_calls,
                            pending_receipts: &mut pending_receipts,
                            unlogged: &mut unlogged,
                            deferred: &mut deferred,
                            limits,
                            updates,
                            record: record.as_deref(),
                            journal: journal.clone(),
                            tools,
                            context,
                            actor_started_at,
                        };
                        calls.resume_deferred();
                    }
                }
                () = async {
                    match watch.stall {
                        Some(stall) => nanocode_platform::sleep(stall.saturating_duration_since(nanocode_platform::now())).await,
                        None => std::future::pending().await,
                    }
                }, if watch.stall.is_some() && !held && !replaying => {
                    return Ok(CellTerminal::Stopped {
                        message: watch.stalled(),
                    });
                }
                () = async {
                    match flushing.as_mut() {
                        Some(flush) => flush.await,
                        None => std::future::pending().await,
                    }
                }, if flushing.is_some() => {
                    flushing = None;
                    if std::mem::take(&mut flush_again) {
                        request_flush(record.as_ref(), &mut flushing, &mut flush_again);
                    }
                }
                event = self.worker.read_event() => {
                    let event = event.map_err(HostFailure::new)?;
                    event_count = event_count.saturating_add(1);
                    if event_count == 1 {
                        nanocode_trace::Span::current().record(
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
                    let mut calls = CellCalls {
                        pending_calls: &mut pending_calls,
                        pending_receipts: &mut pending_receipts,
                        unlogged: &mut unlogged,
                        deferred: &mut deferred,
                        limits,
                        updates,
                        record: record.as_deref(),
                        journal: journal.clone(),
                        tools,
                        context,
                        actor_started_at,
                    };
                    match event {
                        RuntimeEvent::ToolCall {
                            id, name, input, host, ..
                        } => {
                            if held {
                                calls.queue(id, name, input, host);
                            } else {
                                calls.dispatch(id, name, input, host);
                            }
                        }
                        RuntimeEvent::Notify { text, .. } => {
                            if !replaying {
                                let _ = updates.send(CellUpdate::Notification {
                                    notification: CodeModeNotification::new(parent_call_id, text),
                                    delivered: false,
                                });
                            }
                        }
                        RuntimeEvent::Content { content, .. } => {
                            if suppressed > 0 {
                                suppressed -= 1;
                            } else {
                                let _ = updates.send(CellUpdate::Content(content));
                            }
                        }
                        RuntimeEvent::Yielded { .. } => {
                            if !replaying {
                                let _ = updates.send(CellUpdate::Yielded);
                            }
                        }
                        RuntimeEvent::Waiting { .. } => {
                            if !replaying {
                                request_flush(record.as_ref(), &mut flushing, &mut flush_again);
                            }
                        }
                        RuntimeEvent::Held { .. } => {
                            if let Some(record) = record.as_deref() {
                                record.update(|status| status.life = status.life.holding(Hold::Held));
                            }
                        }
                        RuntimeEvent::Replayed { .. } => {
                            replaying = false;
                            if let Some(record) = record.as_deref() {
                                record.update(|status| status.life = status.life.replayed());
                            }
                            if let Some(plan) = plan.take() {
                                self.resume_replayed(cell_id, &plan, held, &mut calls)?;
                            }
                            if held
                                && hold.as_ref().is_some_and(|receiver| !*receiver.borrow())
                                && let Some(record) = record.as_deref()
                            {
                                held = false;
                                self.release_cell(cell_id, record, &mut calls)?;
                            }
                        }
                        RuntimeEvent::Diverged { reason, .. } => {
                            nanocode_trace::warn!(
                                target: "nanocode_code_mode",
                                cell_id,
                                %reason,
                                "a replayed Code Mode cell diverged from its journal"
                            );
                            let message = plan.as_ref().map_or_else(
                                || replay::divergence_text(None, cell_id, &reason),
                                |plan| plan.divergence(cell_id, &reason),
                            );
                            return Ok(CellTerminal::Diverged { message });
                        }
                        RuntimeEvent::Done {
                            stored,
                            ..
                        } => {
                            nanocode_trace::Span::current().record("runtime.event_count", event_count);
                            return Ok(CellTerminal::Completed { stored });
                        }
                        RuntimeEvent::Error {
                            message,
                            stored,
                            ..
                        } => {
                            nanocode_trace::Span::current().record("runtime.event_count", event_count);
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
        record: Option<&CellRecord>,
        held: bool,
    ) -> Result<bool, HostFailure> {
        let CompletedNestedCall {
            id,
            input,
            mut call,
            process_trace,
            receipts,
            encoded,
        } = completed;
        let success = call.success;

        let logged = pending_receipts
            .calls
            .remove(&id)
            .is_none_or(|pending| pending.logged);
        call.input = Arc::unwrap_or_clone(input);
        let encoded = match encoded {
            Some(encoded) => encoded,
            None => serde_json::to_string(&call.structured_result).map_err(|error| {
                HostFailure::new(format!("failed to encode Code Mode tool result: {error}"))
            })?,
        };
        let _ = pending_receipts
            .updates
            .send(CellUpdate::NestedCall(ObservedNestedCall {
                id,
                call,
                process_trace,
                delivered: false,
                logged,
                receipts,
            }));
        if let Some(record) = record {
            let kept = record.update(|status| {
                if !held {
                    status.calls.remove(&id);
                    return false;
                }
                let Some(journaled) = status.calls.get_mut(&id) else {
                    status.unsupported = true;
                    return false;
                };
                Arc::make_mut(journaled).state = JournaledCallState::Completed {
                    encoded: encoded.clone(),
                    success,
                };
                true
            });
            if kept {
                record.journal(id);
                return Ok(true);
            }
        }
        self.worker
            .send_tool_result(cell_id, id, encoded, success)
            .map_err(HostFailure::new)?;
        Ok(false)
    }
}

struct ActorJournaling {
    record: Arc<CellRecord>,
    plan: Option<ReplayPlan>,
    hold: watch::Receiver<bool>,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_cell_actor(
    shared_host: Arc<Slot<EmbeddedHost>>,
    cell_id: u64,
    source: String,
    prelude: Option<Arc<str>>,
    limits: Limits,
    tools: Arc<dyn CodeModeToolset>,
    catalog: Arc<CellTools>,
    context: OwnedToolContext,
    stored: HashMap<String, Arc<RawValue>>,
    shared_stored: Arc<Atom<HashMap<String, Arc<RawValue>>>>,
    updates: mpsc::UnboundedSender<CellUpdate>,
    mut terminate: oneshot::Receiver<()>,
    interrupt: Interrupt,
    lifecycle: Arc<CellLifecycle>,
    journaling: Option<ActorJournaling>,
) {
    let started_at = nanocode_platform::now();
    let host_wait_started_at = nanocode_platform::now();
    let (mut host, reused) = {
        let taken = if cfg!(target_family = "wasm") {
            shared_host.take()
        } else {
            None
        };
        let reused = taken.is_some();
        let host = match taken {
            Some(host) => host,
            None => match spawn_host().await {
                Ok(host) => host,
                Err(message) => {
                    nanocode_trace::Span::current().record("status", "failed");
                    nanocode_trace::Span::current().record("otel.status_code", "ERROR");
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
    nanocode_trace::Span::current().record("host.reused", reused);
    let (record, mut plan, hold) = match journaling {
        Some(journaling) => (
            Some(journaling.record),
            journaling.plan,
            Some(journaling.hold),
        ),
        None => (None, None, None),
    };
    let engine_journaling =
        record
            .as_ref()
            .map(|record| nanocode_code_mode_engine::CellJournaling {
                journal: record.journal.clone(),
                replay: plan.as_mut().map(|plan| std::mem::take(&mut plan.entries)),
                started_at_ms: record.started_at_ms,
            });
    let run = async {
        host.worker
            .start_cell(
                cell_id,
                &source,
                stored,
                catalog,
                prelude,
                engine_journaling,
                interrupt,
            )
            .map_err(HostFailure::new)?;
        host.drive_cell(
            cell_id,
            &context.call_id,
            tools.as_ref(),
            &context,
            &updates,
            started_at,
            record.clone(),
            plan,
            hold,
            limits,
        )
        .await
    };
    let selected = nanocode_sync::select! {
        _ = &mut terminate => {
            None
        }
        terminal = run => Some(terminal),
    };
    let terminal = match selected {
        Some(terminal) if lifecycle.claim_completion() => terminal,
        selected => {
            let termination_started_at = nanocode_platform::now();
            host.worker.terminate().await;
            record_elapsed("host.termination_ns", termination_started_at);
            let stored = match selected {
                Some(Ok(
                    CellTerminal::Completed { stored } | CellTerminal::ScriptFailed { stored, .. },
                )) => stored,
                _ => host.worker.take_stored(),
            };
            Ok(CellTerminal::Terminated { stored })
        }
    };
    let (status, otel_status) = match &terminal {
        Ok(CellTerminal::Completed { .. }) => ("completed", "OK"),
        Ok(CellTerminal::Terminated { .. }) => ("cancelled", "ERROR"),
        Ok(
            CellTerminal::ScriptFailed { .. }
            | CellTerminal::Diverged { .. }
            | CellTerminal::Stopped { .. },
        )
        | Err(_) => ("failed", "ERROR"),
    };
    nanocode_trace::Span::current().record("status", status);
    nanocode_trace::Span::current().record("otel.status_code", otel_status);
    let terminated = matches!(&terminal, Ok(CellTerminal::Terminated { .. }));
    let host_healthy = matches!(
        &terminal,
        Ok(CellTerminal::Completed { .. }
            | CellTerminal::ScriptFailed { .. }
            | CellTerminal::Diverged { .. })
    );
    if let Some(record) = &record
        && matches!(
            &terminal,
            Ok(CellTerminal::Completed { .. }
                | CellTerminal::ScriptFailed { .. }
                | CellTerminal::Stopped { .. })
        )
    {
        record.update(|status| status.life = status.life.finished());
    }
    match terminal {
        Ok(CellTerminal::Completed { stored }) => {
            keep_stored(&shared_stored, record.as_deref(), stored).await;
            let _ = updates.send(CellUpdate::Completed);
        }
        Ok(CellTerminal::ScriptFailed { message, stored }) => {
            keep_stored(&shared_stored, record.as_deref(), stored).await;
            let _ = updates.send(CellUpdate::ScriptFailed { message });
        }
        Ok(CellTerminal::Diverged { message }) => {
            if let Some(record) = &record {
                record.update(|status| status.life = CellLife::Retired);
                record.forget();
            }
            let _ = updates.send(CellUpdate::HostFailed(message));
        }
        Ok(CellTerminal::Terminated { stored }) => {
            keep_stored(&shared_stored, record.as_deref(), stored).await;
            let _ = updates.send(CellUpdate::Terminated);
        }
        Ok(CellTerminal::Stopped { message }) => {
            keep_stored(&shared_stored, record.as_deref(), host.worker.take_stored()).await;
            let _ = updates.send(CellUpdate::ScriptFailed { message });
        }
        Err(failure) => {
            let _ = updates.send(CellUpdate::HostFailed(failure.message));
        }
    }
    if host_healthy {
        if cfg!(target_family = "wasm")
            && let Some(mut displaced) = shared_host.put(host)
        {
            displaced.worker.terminate().await;
        }
    } else if !terminated {
        let termination_started_at = nanocode_platform::now();
        host.worker.terminate().await;
        record_elapsed("host.termination_ns", termination_started_at);
    }
    lifecycle.close();
    tools.cell_observed(&context.call_id, true);
    record_elapsed("duration_ns", started_at);
}

async fn keep_stored(
    shared_stored: &Atom<HashMap<String, Arc<RawValue>>>,
    record: Option<&CellRecord>,
    stored: HashMap<String, Arc<RawValue>>,
) {
    if stored.is_empty() {
        return;
    }
    shared_stored.update(|shared| {
        shared.extend(
            stored
                .iter()
                .map(|(key, value)| (key.clone(), Arc::clone(value))),
        )
    });
    if let Some(record) = record {
        record.save_stored(shared_stored).await;
    }
}

fn record_elapsed(field: &'static str, started_at: Instant) {
    nanocode_trace::Span::current().record(
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

async fn journaled_call(
    journal: Option<Arc<CellRecord>>,
    call: Option<JournaledCall>,
    execution: impl std::future::Future<Output = CompletedNestedCall>,
) -> CompletedNestedCall {
    let (Some(journal), Some(call)) = (journal.filter(|record| record.persistence.is_some()), call)
    else {
        return execution.await;
    };
    journal.persist(vec![call]).await;
    let mut completed = execution.await;
    if completed.encoded.is_none() {
        completed.encoded = serde_json::to_string(&completed.call.structured_result).ok();
    }
    completed
}

#[allow(clippy::too_many_arguments)]
async fn execute_nested_call(
    tools: &dyn CodeModeToolset,
    id: u64,
    name: String,
    host: bool,
    input: Arc<Value>,
    context: &OwnedToolContext,
    cell_started_at: Instant,
    how: Nested,
) -> CompletedNestedCall {
    let started_at = nanocode_platform::now();
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
    let mut execution = match how {
        Nested::Recover => {
            let recovered = match tools.recover_nested(&name, Value::clone(&input), context) {
                Some(recovered) => recovered.await,
                None => None,
            };
            recovered.unwrap_or_else(|| {
                ToolOutput::error(replay::UNKNOWN_OUTCOME)
                    .with_structured_result(replay::unknown_outcome())
            })
        }
        Nested::Refuse(cap) => {
            let text = format!(
                "This cell already called {name} {cap} times, so this call didn't run. Batch the work into fewer calls or split it across cells, or the cell is stopped."
            );
            ToolOutput::error(text.clone()).with_structured_result(serde_json::json!({
                "isError": true, "code": CALL_LIMIT, "retryable": false, "error": text,
            }))
        }
        Nested::Run if host => {
            caught(tools.execute_host_nested(&name, Value::clone(&input), context)).await
        }
        Nested::Run => caught(tools.execute_nested(&name, Value::clone(&input), context)).await,
    };
    let duration_ns = u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let encoded = execution.structured_json().map(str::to_owned);
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
        encoded,
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

async fn caught(call: impl std::future::Future<Output = ToolOutput>) -> ToolOutput {
    std::panic::AssertUnwindSafe(call)
        .catch_unwind()
        .await
        .unwrap_or_else(|_| crate::embedded::tool_panicked())
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
