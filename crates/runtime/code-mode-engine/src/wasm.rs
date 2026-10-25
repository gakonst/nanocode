use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

use rquickjs::{Context, Persistent, Promise, Runtime};
use serde_json::{Value, value::RawValue};
use tokio::sync::mpsc::{self, error::TryRecvError};

use crate::cell::{
    ExecutionState, Rejections, RuntimeEvent, clear_pending, completed_terminal, drain_jobs,
    invoke_timeout, new_state, next_due_timeout, next_timeout_wait, resolve_tool, send_terminal,
    start_execution, track_rejections,
};

pub const DEFAULT_INTERRUPT_CHECKS: u64 = 10_000;

const HOST_STACK_BYTES: usize = 192 * 1024;

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
    failure: Option<String>,
    budget: Rc<Cell<u64>>,
    rejections: Rejections,
    runtime: Runtime,
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
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    events: mpsc::UnboundedReceiver<RuntimeEvent>,
}

impl EmbeddedHost {
    pub fn spawn() -> Result<Self, String> {
        let runtime = Runtime::new()
            .map_err(|error| format!("failed to initialize embedded QuickJS runtime: {error}"))?;
        let here = 0_u8;
        let stack = std::ptr::addr_of!(here) as usize;
        runtime.set_max_stack_size(stack.saturating_sub(HOST_STACK_BYTES).max(64 * 1024));
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
        ENGINES.with_borrow_mut(|engines| {
            engines.insert(
                key,
                Engine {
                    execution: None,
                    failure: None,
                    budget,
                    rejections,
                    runtime,
                },
            )
        });
        let (event_tx, events) = mpsc::unbounded_channel();
        Ok(Self {
            key,
            event_tx,
            events,
        })
    }

    pub fn start_cell(
        &self,
        execution_id: u64,
        source: &str,
        stored: HashMap<String, Arc<RawValue>>,
        tools: Vec<Value>,
        prelude: Option<Arc<str>>,
    ) -> Result<(), String> {
        self.step(|engine| {
            if let Some(previous) = engine.execution.take() {
                previous.close(&engine.runtime);
            }
            let context = Context::full(&engine.runtime)
                .map_err(|error| format!("failed to create embedded QuickJS context: {error}"))?;
            let state = new_state(
                execution_id,
                self.event_tx.clone(),
                stored,
                Rc::clone(&engine.rejections),
            );
            let promise = context.with(|ctx| {
                start_execution(&ctx, &state, source.to_owned(), tools, prelude.as_deref())
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
                    self.step(|engine| {
                        let Some(execution) = engine.execution.as_ref() else {
                            return Ok(());
                        };
                        let Some(id) = next_due_timeout(&execution.state) else {
                            return Ok(());
                        };
                        execution.context.with(|ctx| {
                            invoke_timeout(&ctx, &execution.state, id)?;
                            drain_jobs(&ctx, &execution.state);
                            Ok(())
                        })
                    })?;
                }
            }
        }
    }

    pub async fn terminate(&mut self) {
        let engine = ENGINES
            .try_with(|engines| engines.borrow_mut().remove(&self.key))
            .ok()
            .flatten();
        if let Some(mut engine) = engine
            && let Some(execution) = engine.execution.take()
        {
            execution.close(&engine.runtime);
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
        let result = settle(&mut engine, result, exhausted);
        if let Err(error) = &result {
            engine.failure.get_or_insert_with(|| error.clone());
        }
        ENGINES.with_borrow_mut(|engines| engines.insert(self.key, engine));
        result
    }
}

fn settle(engine: &mut Engine, result: Result<(), String>, exhausted: bool) -> Result<(), String> {
    let Some(execution) = engine.execution.as_ref() else {
        return result;
    };
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
                .event_tx
                .send(RuntimeEvent::Error {
                    cell_id: state.execution_id,
                    message: EXHAUSTED.to_owned(),
                    stored: HashMap::new(),
                })
                .map_err(|_| "embedded QuickJS execution observer closed".to_owned())
        }
        finished => finished,
    };
    if let Some(execution) = engine.execution.take() {
        execution.close(&engine.runtime);
    }
    finished
}

impl Drop for EmbeddedHost {
    fn drop(&mut self) {
        let engine = ENGINES
            .try_with(|engines| engines.borrow_mut().remove(&self.key))
            .ok()
            .flatten();
        if let Some(mut engine) = engine
            && let Some(execution) = engine.execution.take()
        {
            execution.close(&engine.runtime);
        }
    }
}
