use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use nanocode_platform::Instant;
use rquickjs::{CatchResultExt, Context, Function, Persistent, Promise, Runtime};
use serde_json::{Value, value::RawValue};
use tokio::sync::mpsc::{self, error::TryRecvError};

use crate::cell::{
    ExecutionState, Rejections, RuntimeEvent, begin_execution, clear_pending, completed_terminal,
    drain_jobs, import_prelude, install_bootstrap, install_prelude, invoke_timeout, new_state,
    next_due_timeout, next_timeout_wait, prepare_execution, resolve_tool, run_execution,
    send_terminal, track_rejections,
};

pub const DEFAULT_INTERRUPT_CHECKS: u64 = 10_000;

const HOST_STACK_BYTES: usize = 192 * 1024;

const PREPARE_AFTER: Duration = Duration::from_millis(20);

const EXHAUSTED: &str = "Script stopped: one synchronous run exceeded this host's CPU budget for a cell. Split long loops with await (for example await new Promise((resolve) => setTimeout(resolve, 0))).";

thread_local! {
    static ENGINES: RefCell<HashMap<u64, Engine>> = RefCell::new(HashMap::new());
    static NEXT_ENGINE: Cell<u64> = const { Cell::new(1) };
    static INTERRUPT_CHECKS: Cell<u64> = const { Cell::new(DEFAULT_INTERRUPT_CHECKS) };
}

pub fn set_interrupt_checks(checks: u64) {
    INTERRUPT_CHECKS.set(checks.max(1));
}

struct Engine {
    execution: Option<Execution>,
    retired: Option<Execution>,
    prepared: Option<Prepared>,
    prelude: Option<Option<Arc<str>>>,
    failure: Option<String>,
    budget: Rc<Cell<u64>>,
    rejections: Rejections,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    runtime: Runtime,
}

struct Prepared {
    context: Context,
    state: Rc<RefCell<ExecutionState>>,
    run_cell: Persistent<Function<'static>>,
    prelude: Option<(Arc<str>, Persistent<Function<'static>>)>,
}

impl Engine {
    fn retire(&mut self) {
        if let Some(retired) = self.retired.take() {
            retired.close(&self.runtime);
        }
    }

    fn prepare(&self, prelude: Option<&Arc<str>>) -> Result<Prepared, String> {
        install_prelude(&self.runtime, prelude.map(|source| &**source))?;
        let context = Context::full(&self.runtime)
            .map_err(|error| format!("failed to create embedded QuickJS context: {error}"))?;
        let state = new_state(
            0,
            self.event_tx.clone(),
            HashMap::new(),
            Rc::clone(&self.rejections),
        );
        let (run_cell, prelude) = context.with(|ctx| {
            let run_cell = prepare_execution(&ctx, &state, prelude.is_some())?;
            let prelude = prelude
                .map(|source| {
                    import_prelude(&ctx)
                        .map(|compiled| (Arc::clone(source), Persistent::save(&ctx, compiled)))
                })
                .transpose()?;
            Ok::<_, String>((Persistent::save(&ctx, run_cell), prelude))
        })?;
        Ok(Prepared {
            context,
            state,
            run_cell,
            prelude,
        })
    }
}

fn prepare_later(key: u64) {
    nanocode_platform::spawn_local(Box::pin(async move {
        nanocode_platform::sleep(PREPARE_AFTER).await;
        let Some(mut engine) = ENGINES
            .try_with(|engines| engines.borrow_mut().remove(&key))
            .ok()
            .flatten()
        else {
            return;
        };
        engine.retire();
        if engine.execution.is_none()
            && engine.prepared.is_none()
            && engine.failure.is_none()
            && let Some(prelude) = engine.prelude.clone()
        {
            engine.budget.set(INTERRUPT_CHECKS.get());
            engine.prepared = engine.prepare(prelude.as_ref()).ok();
        }
        ENGINES.with_borrow_mut(|engines| engines.insert(key, engine));
    }));
}

struct Execution {
    id: u64,
    promise: Option<Persistent<Promise<'static>>>,
    state: Rc<RefCell<ExecutionState>>,
    context: Context,
}

impl Execution {
    fn close(mut self, runtime: &Runtime) {
        clear_pending(&self.state);
        self.promise = None;
        drop(self);
        runtime.run_gc();
    }
}

pub struct EmbeddedHost {
    key: u64,
    events: mpsc::UnboundedReceiver<RuntimeEvent>,
}

impl EmbeddedHost {
    pub fn spawn() -> Result<Self, String> {
        let runtime = Runtime::new()
            .map_err(|error| format!("failed to initialize embedded QuickJS runtime: {error}"))?;
        let here = 0_u8;
        let stack = std::ptr::addr_of!(here) as usize;
        runtime.set_max_stack_size(stack.saturating_sub(HOST_STACK_BYTES).max(64 * 1024));
        install_bootstrap(&runtime)?;
        let budget = Rc::new(Cell::new(INTERRUPT_CHECKS.get()));
        let remaining = Rc::clone(&budget);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let left = remaining.get();
            if left == 0 {
                return true;
            }
            remaining.set(left - 1);
            false
        })));
        let rejections = track_rejections(&runtime);
        let key = NEXT_ENGINE.get();
        NEXT_ENGINE.set(key.wrapping_add(1));
        let (event_tx, events) = mpsc::unbounded_channel();
        ENGINES.with_borrow_mut(|engines| {
            engines.insert(
                key,
                Engine {
                    execution: None,
                    retired: None,
                    prepared: None,
                    prelude: None,
                    failure: None,
                    budget,
                    rejections,
                    event_tx,
                    runtime,
                },
            )
        });
        Ok(Self { key, events })
    }

    pub fn start_cell(
        &self,
        execution_id: u64,
        source: &str,
        stored: HashMap<String, Arc<RawValue>>,
        tools: Vec<Value>,
        prelude: Option<Arc<str>>,
        journaling: Option<crate::journal::CellJournaling>,
    ) -> Result<(), String> {
        if journaling
            .as_ref()
            .is_some_and(|journaling| journaling.replay.is_some())
        {
            return Err("this Code Mode host cannot replay a cell".into());
        }
        self.step(|engine| {
            if let Some(previous) = engine.execution.take() {
                previous.close(&engine.runtime);
            }
            engine.retire();
            let prepared = engine
                .prepared
                .take()
                .filter(|prepared| {
                    prepared.prelude.as_ref().map(|(source, _)| source) == prelude.as_ref()
                })
                .map_or_else(|| engine.prepare(prelude.as_ref()), Ok)?;
            engine.prelude = Some(prelude);
            begin_execution(&prepared.state, execution_id, stored, journaling);
            let Prepared {
                context,
                state,
                run_cell,
                prelude,
            } = prepared;
            let promise = context.with(|ctx| {
                let run_cell = run_cell.restore(&ctx).catch(&ctx).map_err(|error| {
                    format!("failed to restore the QuickJS cell runner: {error}")
                })?;
                let prelude = match prelude {
                    Some((_, compiled)) => compiled
                        .restore(&ctx)
                        .catch(&ctx)
                        .map_err(|error| format!("failed to restore the QuickJS prelude: {error}"))?
                        .into_value(),
                    None => rquickjs::Value::new_undefined(ctx.clone()),
                };
                run_execution(&ctx, &state, &run_cell, source.to_owned(), tools, prelude)
                    .map(|promise| Persistent::save(&ctx, promise))
            });
            engine.execution = Some(Execution {
                id: execution_id,
                promise: promise.as_ref().ok().cloned(),
                state,
                context,
            });
            promise.map(drop)
        })
    }

    pub fn send_tool_result(
        &self,
        execution_id: u64,
        id: u64,
        encoded: String,
        success: bool,
    ) -> Result<(), String> {
        self.step(|engine| {
            let Some(execution) = engine
                .execution
                .as_ref()
                .filter(|execution| execution.id == execution_id)
            else {
                return Ok(());
            };
            execution.context.with(|ctx| {
                resolve_tool(&ctx, &execution.state, id, encoded, success)?;
                drain_jobs(&ctx, &execution.state);
                Ok(())
            })
        })
    }

    pub async fn read_event(&mut self) -> Result<RuntimeEvent, String> {
        loop {
            match self.events.try_recv() {
                Ok(event) => return Ok(event),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    return Err("embedded QuickJS code-mode host ended before a result".into());
                }
            }
            if let Some(failure) = self.with_engine(|engine| engine.failure.take()).flatten() {
                return Err(failure);
            }
            let wait = self.with_engine(|engine| {
                engine
                    .execution
                    .as_ref()
                    .and_then(|execution| next_timeout_wait(&execution.state))
            });
            let Some(wait) = wait.flatten() else {
                return self.events.recv().await.ok_or_else(|| {
                    "embedded QuickJS code-mode host ended before a result".to_owned()
                });
            };
            tokio::select! {
                biased;
                event = self.events.recv() => {
                    return event.ok_or_else(|| "embedded QuickJS code-mode host ended before a result".to_owned());
                }
                () = nanocode_platform::sleep(wait) => {
                    let now = Instant::now();
                    let budget = INTERRUPT_CHECKS.get();
                    let mut spent = 0;
                    while spent < budget {
                        let mut ran = false;
                        self.step(|engine| {
                            let Some(execution) = engine.execution.as_ref() else {
                                return Ok(());
                            };
                            let Some(id) = next_due_timeout(&execution.state, now) else {
                                return Ok(());
                            };
                            ran = true;
                            let result = execution.context.with(|ctx| {
                                invoke_timeout(&ctx, &execution.state, id)?;
                                drain_jobs(&ctx, &execution.state);
                                Ok(())
                            });
                            spent += budget - engine.budget.get();
                            result
                        })?;
                        if !ran {
                            break;
                        }
                    }
                }
            }
        }
    }

    pub fn hold(&self, _execution_id: u64) -> Result<(), String> {
        Err("this Code Mode host cannot hold a cell".into())
    }

    pub fn release(&self, _execution_id: u64) -> Result<(), String> {
        Err("this Code Mode host cannot hold a cell".into())
    }

    pub async fn terminate(&mut self) {
        let engine = ENGINES
            .try_with(|engines| engines.borrow_mut().remove(&self.key))
            .ok()
            .flatten();
        if let Some(mut engine) = engine {
            if let Some(execution) = engine.execution.take() {
                execution.close(&engine.runtime);
            }
            engine.retire();
        }
    }

    fn with_engine<T>(&self, run: impl FnOnce(&mut Engine) -> T) -> Option<T> {
        ENGINES.with_borrow_mut(|engines| engines.get_mut(&self.key).map(run))
    }

    fn step(&self, run: impl FnOnce(&mut Engine) -> Result<(), String>) -> Result<(), String> {
        let mut engine = ENGINES
            .with_borrow_mut(|engines| engines.remove(&self.key))
            .ok_or_else(|| "embedded QuickJS code-mode host is unavailable".to_owned())?;
        engine.budget.set(INTERRUPT_CHECKS.get());
        let result = run(&mut engine);
        let exhausted = engine.budget.get() == 0;
        let result = settle(self.key, &mut engine, result, exhausted);
        if let Err(error) = &result {
            engine.failure.get_or_insert_with(|| error.clone());
        }
        ENGINES.with_borrow_mut(|engines| engines.insert(self.key, engine));
        result
    }
}

fn settle(
    key: u64,
    engine: &mut Engine,
    result: Result<(), String>,
    exhausted: bool,
) -> Result<(), String> {
    let Some(execution) = engine.execution.as_ref() else {
        return result;
    };
    execution.state.borrow().flush_text();
    let finished = match (&result, &execution.promise) {
        (Err(error), _) => Some(Err(error.clone())),
        (Ok(()), None) => Some(Err("embedded QuickJS execution has no result".to_owned())),
        (Ok(()), Some(promise)) => execution.context.with(|ctx| {
            let promise = promise
                .clone()
                .restore(&ctx)
                .map_err(|error| format!("failed to restore embedded QuickJS result: {error}"));
            match promise.and_then(|promise| completed_terminal(&ctx, &promise)) {
                Ok(None) => None,
                Ok(Some(terminal)) => Some(send_terminal(&execution.state, terminal)),
                Err(error) => Some(Err(error)),
            }
        }),
    };
    let Some(finished) = finished else {
        return Ok(());
    };
    let finished = match finished {
        Err(_) if exhausted => {
            let state = execution.state.borrow();
            state
                .send(RuntimeEvent::Error {
                    cell_id: state.execution_id,
                    message: EXHAUSTED.to_owned(),
                    stored: HashMap::new(),
                })
                .map_err(|_| "embedded QuickJS execution observer closed".to_owned())
        }
        finished => finished,
    };
    if let Some(mut execution) = engine.execution.take() {
        clear_pending(&execution.state);
        execution.promise = None;
        engine.retire();
        engine.retired = Some(execution);
        prepare_later(key);
    }
    finished
}

impl Drop for EmbeddedHost {
    fn drop(&mut self) {
        let engine = ENGINES
            .try_with(|engines| engines.borrow_mut().remove(&self.key))
            .ok()
            .flatten();
        if let Some(mut engine) = engine {
            if let Some(execution) = engine.execution.take() {
                execution.close(&engine.runtime);
            }
            engine.retire();
        }
    }
}
