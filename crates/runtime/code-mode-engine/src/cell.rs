use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc, time::Duration};

use nanocode_platform::Instant;
use rquickjs::{
    CatchResultExt, Coerced, Ctx, Exception, FromJs, Function, Persistent, Promise, Runtime,
    context::EvalOptions,
    function::{Func, Opt, Rest},
    promise::PromiseState,
};
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use tokio::sync::mpsc;

use nanocode_tool_contracts::ToolOutputContent;

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
}

impl RuntimeEvent {
    pub const fn cell_id(&self) -> u64 {
        match self {
            Self::ToolCall { cell_id, .. }
            | Self::Notify { cell_id, .. }
            | Self::Content { cell_id, .. }
            | Self::Yielded { cell_id, .. }
            | Self::Done { cell_id, .. }
            | Self::Error { cell_id, .. } => *cell_id,
        }
    }
}

pub(crate) const BOOTSTRAP: &str = include_str!("bootstrap.js");

pub(crate) const CODE_TOOLS: &str = include_str!("code-tools.mjs");
pub(crate) const CODE_VALUES: &str = include_str!("code-values.mjs");

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
    next_tool_id: u64,
    next_timeout_id: u64,
    rejections: Rejections,
    uncaught: Option<SavedFunction>,
    failure: Option<String>,
    pending_text: RefCell<Option<String>>,
}

const PENDING_TEXT_BYTES: usize = 64 * 1024;

impl ExecutionState {
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
        next_tool_id: 1,
        next_timeout_id: 1,
        rejections,
        uncaught: None,
        failure: None,
        pending_text: RefCell::new(None),
    }))
}

#[cfg(target_family = "wasm")]
pub(crate) fn begin_execution(
    state: &Rc<RefCell<ExecutionState>>,
    execution_id: u64,
    stored: HashMap<String, Arc<RawValue>>,
) {
    let mut state = state.borrow_mut();
    state.rejections.borrow_mut().clear();
    state.execution_id = execution_id;
    state.stored = stored;
}

pub(crate) fn clear_pending(state: &Rc<RefCell<ExecutionState>>) {
    let mut state = state.borrow_mut();
    state.pending_tools.clear();
    state.pending_timeouts.clear();
    state.uncaught = None;
    let rejections = std::mem::take(&mut *state.rejections.borrow_mut());
    drop(state);
    drop(rejections);
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn start_execution<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<RefCell<ExecutionState>>,
    source: String,
    tools: Vec<Value>,
    prelude: Option<&str>,
) -> Result<Promise<'js>, String> {
    let run_cell = prepare_execution(ctx, state, prelude.is_some())?;
    let prelude = prelude.map_or_else(
        || Ok(rquickjs::Value::new_undefined(ctx.clone())),
        |prelude| {
            rquickjs::String::from_str(ctx.clone(), prelude)
                .map(rquickjs::String::into_value)
                .catch(ctx)
                .map_err(|error| format!("failed to pass the QuickJS prelude: {error}"))
        },
    )?;
    run_execution(ctx, state, &run_cell, source, tools, prelude)
}

pub(crate) fn prepare_execution<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<RefCell<ExecutionState>>,
    bytes: bool,
) -> Result<Function<'js>, String> {
    install_native_functions(ctx, state)?;

    let factory_source = format!(
        "({})",
        CODE_TOOLS.replacen("export function", "function", 1)
    );
    let create_tools = ctx
        .eval::<Function<'js>, _>(factory_source)
        .catch(ctx)
        .map_err(|error| format!("failed to evaluate shared tool facade: {error}"))?;
    let values_factory = CODE_VALUES
        .split_once("export const")
        .ok_or_else(|| "shared value helper export marker is missing".to_owned())?
        .0;
    let value_helpers = ctx
        .eval::<rquickjs::Object<'js>, _>(format!(
            "(() => {{{values_factory}\nreturn createValueHelpers();}})()"
        ))
        .catch(ctx)
        .map_err(|error| format!("failed to evaluate shared value helpers: {error}"))?;
    let bootstrap = ctx
        .eval::<Function<'js>, _>(BOOTSTRAP)
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
        crate::bytes::install(ctx)?;
    }
    Ok(run_cell)
}

#[cfg(target_family = "wasm")]
pub(crate) fn compile_prelude<'js>(
    ctx: &Ctx<'js>,
    run_cell: &Function<'js>,
    prelude: &str,
) -> Result<Function<'js>, String> {
    run_cell
        .get::<_, Function<'js>>("compile")
        .and_then(|compile| compile.call((prelude,)))
        .catch(ctx)
        .map_err(|error| format!("failed to compile the QuickJS prelude: {error}"))
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
                    let _ = state.send(RuntimeEvent::ToolCall {
                        cell_id: state.execution_id,
                        id,
                        name,
                        input,
                        host: host.0.unwrap_or(false),
                    });
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
                    state.pending_timeouts.insert(
                        id,
                        PendingTimeout {
                            callback: Persistent::save(&ctx, callback),
                            deadline: Instant::now().checked_add(Duration::from_millis(delay_ms)),
                        },
                    );
                    Ok(id)
                },
            ),
        )
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS timer callback: {error}"))?;

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
                        clear_timeout_state
                            .borrow_mut()
                            .pending_timeouts
                            .remove(&normalize_delay_ms(id));
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
        "__nanocodeLoad",
        "__nanocodeNotify",
        "__nanocodeScript",
        "__nanocodeYield",
        "__nanocodeSetTimeout",
        "__nanocodeClearTimeout",
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
    let timeout = state.borrow_mut().pending_timeouts.remove(&id);
    let Some(timeout) = timeout else {
        return Ok(());
    };
    let callback = timeout
        .callback
        .restore(ctx)
        .map_err(|error| format!("failed to restore QuickJS timeout callback: {error}"))?;
    callback
        .call::<_, ()>(())
        .catch(ctx)
        .map_err(|error| format!("embedded QuickJS timeout callback failed: {error}"))
}

pub(crate) fn next_timeout_wait(state: &Rc<RefCell<ExecutionState>>) -> Option<Duration> {
    let now = Instant::now();
    state
        .borrow()
        .pending_timeouts
        .values()
        .filter_map(|timeout| timeout.deadline)
        .min()
        .map(|deadline| deadline.saturating_duration_since(now))
}

pub(crate) fn next_due_timeout(state: &Rc<RefCell<ExecutionState>>) -> Option<u64> {
    let now = Instant::now();
    state
        .borrow()
        .pending_timeouts
        .iter()
        .filter_map(|(id, timeout)| {
            timeout
                .deadline
                .filter(|deadline| *deadline <= now)
                .map(|deadline| (*id, deadline))
        })
        .min_by_key(|(id, deadline)| (*deadline, *id))
        .map(|(id, _)| id)
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
