use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    rc::Rc,
    sync::{Arc, OnceLock},
    time::Duration,
};

use nanocode_platform::Instant;
use nanocode_sync::Atom;
use rquickjs::{
    CatchResultExt, Coerced, Context, Ctx, Exception, FromJs, Function, Module, Object, Persistent,
    Promise, Runtime, WriteOptions,
    context::EvalOptions,
    function::{Func, Opt, Rest},
    loader::{ImportAttributes, Resolver, bundle::Bundle},
    promise::PromiseState,
};
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use tokio::sync::mpsc;

use nanocode_tool_contracts::ToolOutputContent;

use crate::journal::{CellJournaling, JournalEntry, SharedJournal, lock as lock_journal};

pub enum RuntimeEvent {
    ToolCall {
        cell_id: u64,
        id: u64,
        name: String,
        input: Value,
        host: bool,
    },
    Notify {
        cell_id: u64,
        text: String,
    },
    Content {
        cell_id: u64,
        content: ToolOutputContent,
    },
    Yielded {
        cell_id: u64,
    },
    Done {
        cell_id: u64,
        stored: HashMap<String, Arc<RawValue>>,
    },
    Error {
        cell_id: u64,
        message: String,
        stored: HashMap<String, Arc<RawValue>>,
    },
    Held {
        cell_id: u64,
    },
    Replayed {
        cell_id: u64,
    },
    Diverged {
        cell_id: u64,
        reason: String,
    },
}

impl RuntimeEvent {
    pub const fn cell_id(&self) -> u64 {
        match self {
            Self::ToolCall { cell_id, .. }
            | Self::Notify { cell_id, .. }
            | Self::Content { cell_id, .. }
            | Self::Yielded { cell_id, .. }
            | Self::Done { cell_id, .. }
            | Self::Error { cell_id, .. }
            | Self::Held { cell_id }
            | Self::Replayed { cell_id }
            | Self::Diverged { cell_id, .. } => *cell_id,
        }
    }
}

pub(crate) const BOOTSTRAP: &str = include_str!("bootstrap.js");

pub(crate) const CODE_TOOLS: &str = include_str!("code-tools.mjs");
pub(crate) const CODE_VALUES: &str = include_str!("code-values.mjs");

const BOOTSTRAP_MODULE: &str = "nanocode:bootstrap";
const PRELUDE_MODULE: &str = "nanocode:prelude";

type BootstrapBundle = &'static [(&'static str, &'static [u8])];

thread_local! {
    static LOADING_BOOTSTRAP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct BootstrapResolver;

impl Resolver for BootstrapResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        if (name == BOOTSTRAP_MODULE || name == PRELUDE_MODULE) && LOADING_BOOTSTRAP.get() {
            Ok(name.to_owned())
        } else {
            Err(rquickjs::Error::new_resolving(base, name))
        }
    }
}

fn bootstrap_bundle(runtime: &Runtime) -> Result<BootstrapBundle, String> {
    static BUNDLE: OnceLock<Result<BootstrapBundle, String>> = OnceLock::new();
    BUNDLE.get_or_init(|| compile_bootstrap(runtime)).clone()
}

pub(crate) fn install_bootstrap(runtime: &Runtime) -> Result<(), String> {
    install_prelude(runtime, None)
}

pub(crate) fn install_prelude(runtime: &Runtime, prelude: Option<&str>) -> Result<(), String> {
    let bootstrap = bootstrap_bundle(runtime)?;
    let bundle = match prelude {
        Some(source) => prelude_bundle(runtime, bootstrap, source)?,
        None => bootstrap,
    };
    runtime.set_loader(BootstrapResolver, Bundle(bundle));
    Ok(())
}

fn prelude_bundle(
    runtime: &Runtime,
    bootstrap: BootstrapBundle,
    source: &str,
) -> Result<BootstrapBundle, String> {
    type Preludes = Vec<(Arc<str>, BootstrapBundle)>;
    static PRELUDES: OnceLock<Atom<Preludes>> = OnceLock::new();
    let preludes = PRELUDES.get_or_init(Atom::default);
    let find = |list: &Preludes| {
        list.iter()
            .find(|(text, _)| **text == *source)
            .map(|(_, bundle)| *bundle)
    };
    if let Some(bundle) = preludes.read(find) {
        return Ok(bundle);
    }
    let bytecode = compile_module(
        runtime,
        PRELUDE_MODULE,
        format!("export default function (tools, call) {{\n{source}\n}}\n"),
    )
    .map_err(|error| format!("failed to compile the QuickJS prelude: {error}"))?;
    let bundle: BootstrapBundle = Box::leak(Box::new([
        bootstrap[0],
        (PRELUDE_MODULE, &*Box::leak(bytecode.into_boxed_slice())),
    ]));
    let source: Arc<str> = Arc::from(source);
    Ok(preludes.update(|list| {
        find(list).unwrap_or_else(|| {
            list.push((Arc::clone(&source), bundle));
            bundle
        })
    }))
}

fn compile_module(runtime: &Runtime, name: &str, source: String) -> Result<Vec<u8>, String> {
    let context = Context::full(runtime).map_err(|error| error.to_string())?;
    context.with(|ctx| {
        Module::declare(ctx.clone(), name, source)
            .and_then(|module| {
                module.write(WriteOptions {
                    strip_source: true,
                    ..WriteOptions::default()
                })
            })
            .catch(&ctx)
            .map_err(|error| error.to_string())
    })
}

fn compile_bootstrap(runtime: &Runtime) -> Result<BootstrapBundle, String> {
    let values = CODE_VALUES
        .split_once("export const")
        .ok_or_else(|| "shared value helper export marker is missing".to_owned())?
        .0;
    let source = format!(
        "export const bootstrap = {};\n{CODE_TOOLS}\n{values}\nexport {{ createValueHelpers }};\n",
        BOOTSTRAP.trim_end()
    );
    let bytecode = compile_module(runtime, BOOTSTRAP_MODULE, source)
        .map_err(|error| format!("failed to compile the embedded QuickJS bootstrap: {error}"))?;
    let bytecode: &'static [u8] = Box::leak(bytecode.into_boxed_slice());
    Ok(Box::leak(Box::new([(BOOTSTRAP_MODULE, bytecode)])))
}

pub(crate) type SavedFunction = Persistent<Function<'static>>;

pub(crate) type Rejections = Rc<RefCell<Vec<Rejection>>>;

pub(crate) struct Rejection {
    promise: Persistent<rquickjs::Value<'static>>,
    reason: Persistent<rquickjs::Value<'static>>,
}

pub(crate) fn track_rejections(runtime: &Runtime) -> Rejections {
    let rejections = Rejections::default();
    let tracked = Rc::clone(&rejections);
    runtime.set_host_promise_rejection_tracker(Some(Box::new(
        move |ctx, promise, reason, handled| {
            let Ok(mut rejections) = tracked.try_borrow_mut() else {
                return;
            };
            if handled {
                rejections.retain(|rejection| {
                    rejection
                        .promise
                        .clone()
                        .restore(&ctx)
                        .is_ok_and(|rejected| rejected != promise)
                });
            } else {
                rejections.push(Rejection {
                    promise: Persistent::save(&ctx, promise),
                    reason: Persistent::save(&ctx, reason),
                });
            }
        },
    )));
    rejections
}

pub(crate) struct ExecutionState {
    pub(crate) execution_id: u64,
    pub(crate) event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    stored: HashMap<String, Arc<RawValue>>,
    pending_tools: HashMap<u64, (SavedFunction, SavedFunction)>,
    pending_timeouts: HashMap<u64, PendingTimeout>,
    timeout_order: BTreeSet<(Instant, u64)>,
    next_tool_id: u64,
    next_timeout_id: u64,
    rejections: Rejections,
    uncaught: Option<SavedFunction>,
    failure: Option<String>,
    pending_text: RefCell<Option<String>>,
    journal: Option<SharedJournal>,
    replay: Option<Replay>,
}

pub(crate) struct Replay {
    entries: Vec<JournalEntry>,
    pos: usize,
    diverged: Option<String>,
}

pub(crate) enum Expectation {
    Live(JournalEntry),
    Replayed(JournalEntry),
    Diverged,
}

const PENDING_TEXT_BYTES: usize = 64 * 1024;

impl ExecutionState {
    const fn journaling(&self) -> bool {
        self.journal.is_some() || self.replay.is_some()
    }

    fn record(&self, entry: JournalEntry, weight: usize) {
        if let Some(journal) = &self.journal {
            lock_journal(journal).record(entry, weight);
        }
    }

    pub(crate) fn expect(&mut self, wanted: JournalEntry, weight: usize) -> Expectation {
        if self
            .replay
            .as_ref()
            .is_some_and(|replay| replay.diverged.is_none() && replay.pos >= replay.entries.len())
        {
            self.finish_replay();
        }
        let Some(replay) = self.replay.as_mut() else {
            if self.journal.is_some() {
                self.record(wanted.clone(), weight);
            }
            return Expectation::Live(wanted);
        };
        if replay.diverged.is_some() {
            return Expectation::Diverged;
        }
        match replay.entries.get(replay.pos).cloned() {
            Some(found) if found.same_step(&wanted) => {
                replay.pos += 1;
                self.record(found.clone(), weight);
                Expectation::Replayed(found)
            }
            found => {
                replay.diverged = Some(divergence(replay.pos, found.as_ref(), &wanted));
                Expectation::Diverged
            }
        }
    }

    pub(crate) fn finish_replay(&mut self) {
        if self.replay.take().is_none() {
            return;
        }
        let now = Instant::now();
        let wall = wall_ms();
        let mut scheduled = Vec::new();
        for (id, timeout) in &mut self.pending_timeouts {
            let Some((at_ms, delay_ms)) = timeout.planned.take() else {
                continue;
            };
            let due = at_ms.saturating_add(delay_ms).saturating_sub(wall);
            timeout.deadline = now.checked_add(Duration::from_millis(due));
            if let Some(deadline) = timeout.deadline {
                scheduled.push((deadline, *id));
            }
        }
        self.timeout_order.extend(scheduled);
        let _ = self.send(RuntimeEvent::Replayed {
            cell_id: self.execution_id,
        });
    }

    pub(crate) fn random_bytes(&mut self, live: Vec<u8>) -> Vec<u8> {
        if !self.journaling() {
            return live;
        }
        let length = live.len();
        match self.expect(JournalEntry::Bytes { data: live }, length) {
            Expectation::Live(JournalEntry::Bytes { data })
            | Expectation::Replayed(JournalEntry::Bytes { data }) => data,
            _ => vec![0; length],
        }
    }

    pub(crate) fn send(
        &self,
        event: RuntimeEvent,
    ) -> Result<(), mpsc::error::SendError<RuntimeEvent>> {
        self.flush_text();
        self.event_tx.send(event)
    }

    pub(crate) fn flush_text(&self) {
        let Some(text) = self.pending_text.borrow_mut().take() else {
            return;
        };
        let _ = self.event_tx.send(RuntimeEvent::Content {
            cell_id: self.execution_id,
            content: ToolOutputContent::InputText { text },
        });
    }

    fn push_text(&self, text: String) {
        let full = {
            let mut pending = self.pending_text.borrow_mut();
            match pending.as_mut() {
                Some(buffered) => {
                    buffered.push('\n');
                    buffered.push_str(&text);
                }
                None => *pending = Some(text),
            }
            pending
                .as_ref()
                .is_some_and(|buffered| buffered.len() >= PENDING_TEXT_BYTES)
        };
        if full {
            self.flush_text();
        }
    }
}

pub(crate) struct PendingTimeout {
    callback: SavedFunction,
    deadline: Option<Instant>,
    planned: Option<(u64, u64)>,
}

fn wall_ms() -> u64 {
    nanocode_platform::system_time()
        .duration_since(nanocode_platform::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn divergence(position: usize, found: Option<&JournalEntry>, wanted: &JournalEntry) -> String {
    match found {
        Some(found) => format!(
            "at journal entry {position} the script reached {} where the journal recorded {}",
            wanted.describe(),
            found.describe()
        ),
        None => format!(
            "the script reached {} past the journal's end",
            wanted.describe()
        ),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalKind {
    Done,
    Error,
}

#[derive(Deserialize)]
pub(crate) struct ExecutionTerminal {
    #[serde(rename = "type")]
    kind: TerminalKind,
    message: Option<String>,
    #[serde(default)]
    stored: HashMap<String, Box<RawValue>>,
}

pub(crate) fn new_state(
    execution_id: u64,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    stored: HashMap<String, Arc<RawValue>>,
    rejections: Rejections,
) -> Rc<RefCell<ExecutionState>> {
    rejections.borrow_mut().clear();
    Rc::new(RefCell::new(ExecutionState {
        execution_id,
        event_tx,
        stored,
        pending_tools: HashMap::new(),
        pending_timeouts: HashMap::new(),
        timeout_order: BTreeSet::new(),
        next_tool_id: 1,
        next_timeout_id: 1,
        rejections,
        uncaught: None,
        failure: None,
        pending_text: RefCell::new(None),
        journal: None,
        replay: None,
    }))
}

pub(crate) fn begin_execution(
    state: &Rc<RefCell<ExecutionState>>,
    execution_id: u64,
    stored: HashMap<String, Arc<RawValue>>,
    journaling: Option<CellJournaling>,
) {
    let mut state = state.borrow_mut();
    state.rejections.borrow_mut().clear();
    state.execution_id = execution_id;
    state.stored = stored;
    let (journal, replay) = journaling.map_or((None, None), |journaling| {
        (
            Some(journaling.journal),
            journaling.replay.map(|entries| Replay {
                entries,
                pos: 0,
                diverged: None,
            }),
        )
    });
    state.journal = journal;
    state.replay = replay;
}

pub(crate) fn clear_pending(state: &Rc<RefCell<ExecutionState>>) {
    let mut state = state.borrow_mut();
    state.pending_tools.clear();
    state.pending_timeouts.clear();
    state.timeout_order.clear();
    state.uncaught = None;
    state.journal = None;
    state.replay = None;
    let rejections = std::mem::take(&mut *state.rejections.borrow_mut());
    drop(state);
    drop(rejections);
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn start_execution<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<RefCell<ExecutionState>>,
    run_cell: &Function<'js>,
    source: String,
    tools: Vec<Value>,
    prelude: bool,
) -> Result<Promise<'js>, String> {
    let prelude = if prelude {
        import_prelude(ctx)?.into_value()
    } else {
        rquickjs::Value::new_undefined(ctx.clone())
    };
    run_execution(ctx, state, run_cell, source, tools, prelude)
}

pub(crate) fn prepare_execution<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<RefCell<ExecutionState>>,
    bytes: bool,
) -> Result<Function<'js>, String> {
    install_native_functions(ctx, state)?;

    LOADING_BOOTSTRAP.set(true);
    let module = Module::import(ctx, BOOTSTRAP_MODULE)
        .and_then(|imported| imported.finish::<Object<'js>>())
        .catch(ctx)
        .map_err(|error| format!("failed to load the embedded QuickJS bootstrap: {error}"));
    LOADING_BOOTSTRAP.set(false);
    let module = module?;
    let create_tools = module
        .get::<_, Function<'js>>("createCodeTools")
        .catch(ctx)
        .map_err(|error| format!("failed to evaluate shared tool facade: {error}"))?;
    let value_helpers = module
        .get::<_, Function<'js>>("createValueHelpers")
        .and_then(|create| create.call::<_, Object<'js>>(()))
        .catch(ctx)
        .map_err(|error| format!("failed to evaluate shared value helpers: {error}"))?;
    let bootstrap = module
        .get::<_, Function<'js>>("bootstrap")
        .catch(ctx)
        .map_err(|error| format!("failed to evaluate embedded QuickJS bootstrap: {error}"))?;
    let run_cell = bootstrap
        .call::<_, Function<'js>>((create_tools, value_helpers))
        .catch(ctx)
        .map_err(|error| format!("failed to initialize embedded QuickJS bootstrap: {error}"))?;
    let uncaught = run_cell
        .get::<_, Function<'js>>("uncaught")
        .catch(ctx)
        .map_err(|error| format!("failed to read the QuickJS rejection handler: {error}"))?;
    state.borrow_mut().uncaught = Some(Persistent::save(ctx, uncaught));
    remove_native_globals(ctx)?;
    if bytes {
        crate::bytes::install(ctx, state)?;
    }
    Ok(run_cell)
}

pub(crate) fn import_prelude<'js>(ctx: &Ctx<'js>) -> Result<Function<'js>, String> {
    LOADING_BOOTSTRAP.set(true);
    let prelude = Module::import(ctx, PRELUDE_MODULE)
        .and_then(|imported| imported.finish::<Object<'js>>())
        .and_then(|module| module.get::<_, Function<'js>>("default"))
        .catch(ctx)
        .map_err(|error| format!("failed to load the QuickJS prelude: {error}"));
    LOADING_BOOTSTRAP.set(false);
    prelude
}

pub(crate) fn run_execution<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<RefCell<ExecutionState>>,
    run_cell: &Function<'js>,
    source: String,
    tools: Vec<Value>,
    prelude: rquickjs::Value<'js>,
) -> Result<Promise<'js>, String> {
    let (tools, schemas) = split_schemas(tools);
    let tools = serde_json::to_string(&tools)
        .map_err(|error| format!("failed to encode QuickJS tool metadata: {error}"))?;
    let schemas = serde_json::to_string(&schemas)
        .map_err(|error| format!("failed to encode QuickJS tool schemas: {error}"))?;
    let promise = run_cell
        .call::<_, Promise<'js>>((source, tools, schemas, prelude))
        .catch(ctx)
        .map_err(|error| format!("embedded QuickJS execution failed to start: {error}"))?;
    drain_jobs(ctx, state);
    Ok(promise)
}

pub(crate) fn split_schemas(mut tools: Vec<Value>) -> (Vec<Value>, Vec<[Value; 2]>) {
    let schemas = tools
        .iter_mut()
        .map(|tool| match tool {
            Value::Object(fields) => [
                fields.remove("input_schema").unwrap_or(Value::Null),
                fields.remove("output_schema").unwrap_or(Value::Null),
            ],
            _ => [Value::Null, Value::Null],
        })
        .collect();
    (tools, schemas)
}

#[allow(clippy::too_many_lines)]
pub(crate) fn install_native_functions<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<RefCell<ExecutionState>>,
) -> Result<(), String> {
    let globals = ctx.globals();

    let tool_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeTool",
            Func::from(
                move |ctx: Ctx<'js>,
                      name: String,
                      input_json: String,
                      host: Opt<bool>|
                      -> rquickjs::Result<Promise<'js>> {
                    let input = serde_json::from_str(&input_json).map_err(|error| {
                        Exception::throw_type(&ctx, &format!("invalid tool input: {error}"))
                    })?;
                    let (promise, resolve, reject) = Promise::new(&ctx)?;
                    let mut state = tool_state.borrow_mut();
                    let id = state.next_tool_id;
                    state.next_tool_id = state.next_tool_id.saturating_add(1);
                    state.pending_tools.insert(
                        id,
                        (
                            Persistent::save(&ctx, resolve),
                            Persistent::save(&ctx, reject),
                        ),
                    );
                    let host = host.0.unwrap_or(false);
                    if !state.journaling() {
                        let _ = state.send(RuntimeEvent::ToolCall {
                            cell_id: state.execution_id,
                            id,
                            name,
                            input,
                            host,
                        });
                        return Ok(promise);
                    }
                    let weight = name.len().saturating_add(input_json.len());
                    match state.expect(
                        JournalEntry::Call {
                            id,
                            name,
                            input,
                            host,
                        },
                        weight,
                    ) {
                        Expectation::Live(JournalEntry::Call {
                            id,
                            name,
                            input,
                            host,
                        }) => {
                            let _ = state.send(RuntimeEvent::ToolCall {
                                cell_id: state.execution_id,
                                id,
                                name,
                                input,
                                host,
                            });
                        }
                        Expectation::Live(_) | Expectation::Replayed(_) => state.flush_text(),
                        Expectation::Diverged => {}
                    }
                    Ok(promise)
                },
            ),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS tool callback: {error}"))?;

    let content_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeContent",
            Func::from(
                move |ctx: Ctx<'js>, content_json: String| -> rquickjs::Result<()> {
                    let content: ToolOutputContent = serde_json::from_str(&content_json).map_err(|error| {
                        Exception::throw_type(&ctx, &format!("invalid output content: {error}"))
                    })?;

                    let content = match content {
                        ToolOutputContent::InputAudio { ref audio_url }
                            if crate::audio::wav_duration_seconds(audio_url)
                                .is_some_and(|duration| duration < 0.025) =>
                        {
                            ToolOutputContent::InputText {
                                text: "Audio output omitted because the clip is shorter than 25 ms; use a longer clip.".to_owned(),
                            }
                        }
                        content => content,
                    };
                    let state = content_state.borrow();
                    match content {
                        ToolOutputContent::InputText { text } => state.push_text(text),
                        content => {
                            let _ = state.send(RuntimeEvent::Content {
                                cell_id: state.execution_id,
                                content,
                            });
                        }
                    }
                    Ok(())
                },
            ),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS content callback: {error}"))?;

    let text_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeText",
            Func::from(move |text: String| text_state.borrow().push_text(text)),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS text callback: {error}"))?;

    let load_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeLoad",
            Func::from(
                move |ctx: Ctx<'js>,
                      key: String|
                      -> rquickjs::Result<Option<rquickjs::String<'js>>> {
                    load_state
                        .borrow()
                        .stored
                        .get(&key)
                        .map(|value| rquickjs::String::from_str(ctx, value.get()))
                        .transpose()
                },
            ),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS load callback: {error}"))?;

    globals
        .set(
            "__nanocodeScript",
            Func::from(
                |ctx: Ctx<'js>, source: String| -> rquickjs::Result<Promise<'js>> {
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.strict = false;
                    options.promise = true;
                    options.filename = Some("<input>".to_owned());
                    ctx.eval_with_options(source, options)
                },
            ),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS script callback: {error}"))?;

    let notify_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeNotify",
            Func::from(move |text: String| {
                let state = notify_state.borrow();
                let _ = state.send(RuntimeEvent::Notify {
                    cell_id: state.execution_id,
                    text,
                });
            }),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS notify callback: {error}"))?;

    let yield_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeYield",
            Func::from(move || {
                let state = yield_state.borrow();
                let _ = state.send(RuntimeEvent::Yielded {
                    cell_id: state.execution_id,
                });
            }),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS yield callback: {error}"))?;

    let timeout_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeSetTimeout",
            Func::from(
                move |ctx: Ctx<'js>, args: Rest<rquickjs::Value<'js>>| -> rquickjs::Result<u64> {
                    let callback = args
                        .first()
                        .and_then(|value| value.as_function())
                        .cloned()
                        .ok_or_else(|| {
                            throw_message(&ctx, "setTimeout expects a function callback")
                        })?;
                    let delay_ms = args
                        .get(1)
                        .cloned()
                        .map(|value| Coerced::<f64>::from_js(&ctx, value))
                        .transpose()?
                        .map_or(0.0, |value| value.0);
                    let delay_ms = normalize_delay_ms(delay_ms);
                    let mut state = timeout_state.borrow_mut();
                    let id = state.next_timeout_id;
                    state.next_timeout_id = state.next_timeout_id.saturating_add(1);
                    let mut deadline = Instant::now().checked_add(Duration::from_millis(delay_ms));
                    let mut planned = None;
                    if state.journaling() {
                        let wanted = JournalEntry::Timeout {
                            id,
                            delay_ms,
                            at_ms: wall_ms(),
                        };
                        match state.expect(wanted, 0) {
                            Expectation::Replayed(JournalEntry::Timeout { at_ms, .. }) => {
                                planned = Some((at_ms, delay_ms));
                                deadline = None;
                            }
                            Expectation::Diverged => deadline = None,
                            Expectation::Live(_) | Expectation::Replayed(_) => {}
                        }
                    }
                    if let Some(deadline) = deadline {
                        state.timeout_order.insert((deadline, id));
                    }
                    state.pending_timeouts.insert(
                        id,
                        PendingTimeout {
                            callback: Persistent::save(&ctx, callback),
                            deadline,
                            planned,
                        },
                    );
                    Ok(id)
                },
            ),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS timer callback: {error}"))?;

    let value_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeValue",
            Func::from(move |kind: u8, live: f64| -> f64 {
                let mut state = value_state.borrow_mut();
                if !state.journaling() {
                    return live;
                }
                let wanted = match kind {
                    0 => JournalEntry::Clock { value: live },
                    1 => JournalEntry::Monotonic { value: live },
                    _ => JournalEntry::Random { value: live },
                };
                match state.expect(wanted, 0) {
                    Expectation::Replayed(
                        JournalEntry::Clock { value }
                        | JournalEntry::Monotonic { value }
                        | JournalEntry::Random { value },
                    ) => value,
                    _ => live,
                }
            }),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS value callback: {error}"))?;

    let clear_timeout_state = Rc::clone(state);
    globals
        .set(
            "__nanocodeClearTimeout",
            Func::from(
                move |ctx: Ctx<'js>, args: Rest<rquickjs::Value<'js>>| -> rquickjs::Result<()> {
                    let Some(value) = args
                        .first()
                        .filter(|value| !value.is_null() && !value.is_undefined())
                    else {
                        return Ok(());
                    };
                    let id = match Coerced::<f64>::from_js(&ctx, value.clone()) {
                        Ok(id) => id.0,
                        Err(_) => {
                            let _ = ctx.catch();
                            return Err(throw_message(
                                &ctx,
                                "clearTimeout expects a numeric timeout id",
                            ));
                        }
                    };
                    if id.is_finite() && id > 0.0 {
                        let id = normalize_delay_ms(id);
                        let mut state = clear_timeout_state.borrow_mut();
                        if let Some(deadline) = state
                            .pending_timeouts
                            .remove(&id)
                            .and_then(|timeout| timeout.deadline)
                        {
                            state.timeout_order.remove(&(deadline, id));
                        }
                    }
                    Ok(())
                },
            ),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS timer cleanup: {error}"))?;
    Ok(())
}

pub(crate) fn remove_native_globals(ctx: &Ctx<'_>) -> Result<(), String> {
    let globals = ctx.globals();
    for name in [
        "__nanocodeTool",
        "__nanocodeContent",
        "__nanocodeText",
        "__nanocodeLoad",
        "__nanocodeNotify",
        "__nanocodeScript",
        "__nanocodeYield",
        "__nanocodeSetTimeout",
        "__nanocodeClearTimeout",
        "__nanocodeValue",
        "console",
        "Atomics",
        "SharedArrayBuffer",
        "WebAssembly",
    ] {
        globals
            .remove(name)
            .catch(ctx)
            .map_err(|error| format!("failed to remove QuickJS global `{name}`: {error}"))?;
    }
    Ok(())
}

pub(crate) fn completed_terminal(
    ctx: &Ctx<'_>,
    promise: &Promise<'_>,
) -> Result<Option<ExecutionTerminal>, String> {
    match promise.state() {
        PromiseState::Pending => Ok(None),
        PromiseState::Resolved => {
            let encoded = promise
                .result::<String>()
                .ok_or_else(|| "embedded QuickJS promise lost its result".to_owned())?
                .catch(ctx)
                .map_err(|error| format!("failed to read embedded QuickJS result: {error}"))?;
            serde_json::from_str(&encoded)
                .map(Some)
                .map_err(|error| format!("embedded QuickJS returned an invalid result: {error}"))
        }
        PromiseState::Rejected => {
            let result = promise
                .result::<String>()
                .ok_or_else(|| "embedded QuickJS promise lost its rejection".to_owned())?
                .catch(ctx);
            match result {
                Err(error) => Err(format!("embedded QuickJS execution rejected: {error}")),
                Ok(_) => {
                    Err("rejected embedded QuickJS promise returned a successful value".to_owned())
                }
            }
        }
    }
}

pub(crate) fn send_terminal(
    state: &Rc<RefCell<ExecutionState>>,
    terminal: ExecutionTerminal,
) -> Result<(), String> {
    let mut state = state.borrow_mut();
    let cell_id = state.execution_id;
    let stored = shared_values(terminal.stored);
    let event = match (terminal.kind, terminal.message) {
        (TerminalKind::Done, _) => match state.failure.take() {
            Some(message) => RuntimeEvent::Error {
                cell_id,
                message,
                stored,
            },
            None => RuntimeEvent::Done { cell_id, stored },
        },
        (TerminalKind::Error, Some(message)) => RuntimeEvent::Error {
            cell_id,
            message,
            stored,
        },
        (TerminalKind::Error, None) => {
            return Err("embedded QuickJS returned an error result without a message".to_owned());
        }
    };
    state
        .send(event)
        .map_err(|_| "embedded QuickJS execution observer closed".to_owned())
}

pub(crate) fn shared_values(
    stored: HashMap<String, Box<RawValue>>,
) -> HashMap<String, Arc<RawValue>> {
    stored
        .into_iter()
        .map(|(key, value)| (key, Arc::from(value)))
        .collect()
}

pub(crate) fn resolve_tool(
    ctx: &Ctx<'_>,
    state: &Rc<RefCell<ExecutionState>>,
    id: u64,
    encoded: String,
    success: bool,
) -> Result<(), String> {
    let (resolve, reject) = state
        .borrow_mut()
        .pending_tools
        .remove(&id)
        .ok_or_else(|| format!("embedded QuickJS received a result for unknown tool call {id}"))?;
    {
        let state = state.borrow();
        if state.journal.is_some() {
            let weight = encoded.len();
            state.record(
                JournalEntry::Result {
                    id,
                    encoded: encoded.clone(),
                    success,
                },
                weight,
            );
        }
    }
    let function = if success { resolve } else { reject };
    let function = function
        .restore(ctx)
        .map_err(|error| format!("failed to restore QuickJS tool promise: {error}"))?;
    function
        .call::<_, ()>((encoded,))
        .catch(ctx)
        .map_err(|error| format!("failed to settle QuickJS tool promise: {error}"))
}

pub(crate) fn invoke_timeout(
    ctx: &Ctx<'_>,
    state: &Rc<RefCell<ExecutionState>>,
    id: u64,
) -> Result<(), String> {
    let timeout = {
        let mut state = state.borrow_mut();
        let timeout = state.pending_timeouts.remove(&id);
        if let Some(deadline) = timeout.as_ref().and_then(|timeout| timeout.deadline) {
            state.timeout_order.remove(&(deadline, id));
        }
        timeout
    };
    let Some(timeout) = timeout else {
        return Ok(());
    };
    state.borrow().record(JournalEntry::Timer { id }, 0);
    let callback = timeout
        .callback
        .restore(ctx)
        .map_err(|error| format!("failed to restore QuickJS timeout callback: {error}"))?;
    callback
        .call::<_, ()>(())
        .catch(ctx)
        .map_err(|error| format!("embedded QuickJS timeout callback failed: {error}"))
}

pub(crate) fn replay_inputs(
    ctx: &Ctx<'_>,
    state: &Rc<RefCell<ExecutionState>>,
    promise: &Promise<'_>,
) -> Result<Option<String>, String> {
    loop {
        let (position, next) = {
            let state = state.borrow();
            let Some(replay) = state.replay.as_ref() else {
                return Ok(None);
            };
            if let Some(reason) = &replay.diverged {
                return Ok(Some(reason.clone()));
            }
            (replay.pos, replay.entries.get(replay.pos).cloned())
        };
        let Some(entry) = next else {
            state.borrow_mut().finish_replay();
            return Ok(None);
        };
        if !matches!(promise.state(), PromiseState::Pending) {
            return Ok(Some(format!(
                "the script finished before journal entry {position} ({})",
                entry.describe()
            )));
        }
        let known = {
            let state = state.borrow();
            match &entry {
                JournalEntry::Result { id, .. } => state.pending_tools.contains_key(id),
                JournalEntry::Timer { id } => state.pending_timeouts.contains_key(id),
                _ => false,
            }
        };
        if !known {
            return Ok(Some(format!(
                "the script did not reach {} that the journal recorded at entry {position}",
                entry.describe()
            )));
        }
        if let Some(replay) = state.borrow_mut().replay.as_mut() {
            replay.pos += 1;
        }
        match entry {
            JournalEntry::Result {
                id,
                encoded,
                success,
            } => resolve_tool(ctx, state, id, encoded, success)?,
            JournalEntry::Timer { id } => invoke_timeout(ctx, state, id)?,
            _ => {}
        }
        drain_jobs(ctx, state);
    }
}

pub(crate) fn next_timeout_wait(state: &Rc<RefCell<ExecutionState>>) -> Option<Duration> {
    let now = Instant::now();
    state
        .borrow()
        .timeout_order
        .first()
        .map(|(deadline, _)| deadline.saturating_duration_since(now))
}

pub(crate) fn next_due_timeout(state: &Rc<RefCell<ExecutionState>>, at: Instant) -> Option<u64> {
    state
        .borrow()
        .timeout_order
        .first()
        .filter(|(deadline, _)| *deadline <= at)
        .map(|(_, id)| *id)
}

pub(crate) fn drain_jobs(ctx: &Ctx<'_>, state: &Rc<RefCell<ExecutionState>>) {
    loop {
        while ctx.execute_pending_job() {}
        let (rejections, uncaught) = {
            let state = state.borrow();
            let rejections = std::mem::take(&mut *state.rejections.borrow_mut());
            (rejections, state.uncaught.clone())
        };
        if rejections.is_empty() {
            state.borrow().flush_text();
            return;
        }
        for rejection in rejections {
            let reported = uncaught
                .clone()
                .ok_or(())
                .and_then(|uncaught| uncaught.restore(ctx).map_err(drop))
                .and_then(|uncaught| {
                    let reason = rejection.reason.restore(ctx).map_err(drop)?;
                    uncaught
                        .call::<_, Option<String>>((reason, true))
                        .catch(ctx)
                        .map_err(drop)
                });
            let message = match reported {
                Ok(message) => message,
                Err(()) => Some("Unhandled promise rejection".to_owned()),
            };
            if let Some(message) = message {
                state.borrow_mut().failure.get_or_insert(message);
            }
        }
    }
}

pub(crate) fn normalize_delay_ms(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else {
        value.trunc() as u64
    }
}

pub(crate) fn throw_message(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    match rquickjs::String::from_str(ctx.clone(), message) {
        Ok(message) => ctx.throw(message.into_value()),
        Err(error) => error,
    }
}
