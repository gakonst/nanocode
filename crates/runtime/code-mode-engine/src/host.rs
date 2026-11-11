use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use djs_embed::{CallId, Instance, Limits, Step, Value};
use nanocode_platform::Instant;
use serde_json::value::RawValue;

use crate::{
    cell::{
        BOOTSTRAP_MODULE, CellTools, ExecutionState, NEXT, RUN_CELL, Replaying, Reply,
        RuntimeEvent, Unprepared, snapshot,
    },
    journal::{CellJournaling, JournalEntry},
};

#[cfg(target_family = "wasm")]
pub const DEFAULT_INTERRUPT_CHECKS: u64 = 10_000;

#[cfg(target_family = "wasm")]
const EXHAUSTED: &str = "Script stopped: one synchronous run exceeded this host's CPU budget for a cell. Split long loops with await (for example await new Promise((resolve) => setTimeout(resolve, 0))).";

#[cfg(target_family = "wasm")]
thread_local! {
    static INTERRUPT_CHECKS: std::cell::Cell<u64> = const { std::cell::Cell::new(DEFAULT_INTERRUPT_CHECKS) };
}

#[cfg(target_family = "wasm")]
pub fn set_interrupt_checks(checks: u64) {
    INTERRUPT_CHECKS.set(checks.max(1));
}

const SPLITMIX_INCREMENT: u64 = 0x9E37_79B9_7F4A_7C15;
const SPLITMIX_FIRST: u64 = 0xBF58_476D_1CE4_E5B9;
const SPLITMIX_SECOND: u64 = 0x94D0_49BB_1331_11EB;

fn random_seed() -> Result<u64, String> {
    static ENTROPY: OnceLock<u64> = OnceLock::new();
    static CELLS: AtomicU64 = AtomicU64::new(0);
    let entropy = match ENTROPY.get() {
        Some(entropy) => *entropy,
        None => {
            let mut seed = [0; 8];
            nanocode_platform::fill_random(&mut seed)
                .map_err(|error| format!("embedded Code Mode has no randomness: {error}"))?;
            *ENTROPY.get_or_init(|| u64::from_le_bytes(seed))
        }
    };
    let cell = CELLS.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let mut mixed = entropy.wrapping_add(cell.wrapping_mul(SPLITMIX_INCREMENT));
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(SPLITMIX_FIRST);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(SPLITMIX_SECOND);
    Ok(mixed ^ (mixed >> 31))
}

enum Phase {
    Running,
    Idle,
}

#[derive(Clone, Copy)]
enum Settled {
    Checkpoint,
    Waiting,
}

enum Ran {
    Sliced,
    Idle,
    Finished,
}

struct Machine {
    instance: Instance,
    pump: Option<CallId>,
    phase: Phase,
    results: VecDeque<(u64, String, bool)>,
    held: bool,
    settled: Option<Settled>,
    #[cfg(target_family = "wasm")]
    slices: u64,
}

impl Machine {
    fn run(&mut self, state: &mut ExecutionState) -> Result<Ran, String> {
        let slice = self.instance.limits().slice;
        let started = self.instance.fuel_used();
        loop {
            let step = match self.instance.step() {
                Ok(step) => step,
                Err(djs_embed::Error::OutOfMemory) => {
                    state.send(RuntimeEvent::Error {
                        cell_id: state.execution_id,
                        message: "Script stopped: the cell ran out of memory.".to_owned(),
                        stored: HashMap::new(),
                    });
                    return Ok(Ran::Finished);
                }
                Err(error) => return Err(failure(error)),
            };
            match step {
                Step::Yield => return Ok(Ran::Sliced),
                Step::HostCall(id) => {
                    let call = self.instance.host_call(id).map_err(failure)?;
                    let reply = state.host_call(&call);
                    let reply = match &reply {
                        Ok(Reply::Nothing) => Ok(Value::Undefined),
                        Ok(Reply::Number(number)) => Ok(Value::Number(*number)),
                        Ok(Reply::Text(text)) => Ok(Value::Str(text)),
                        Ok(Reply::Lossy(bytes)) => Ok(Value::Utf8(bytes)),
                        Ok(Reply::Bytes(bytes)) => Ok(Value::Bytes(bytes)),
                        Err(throw) => Err(throw.clone()),
                    };
                    call.reply(reply).map_err(failure)?;
                    if self.instance.fuel_used().saturating_sub(started) >= slice {
                        return Ok(Ran::Sliced);
                    }
                }
                Step::Wait(started) => {
                    if !started.is_empty() {
                        for id in started {
                            if self.instance.host_call(id).map_err(failure)?.fn_id() == NEXT {
                                self.pump = Some(id);
                            }
                        }
                        continue;
                    }
                    let pump = self.pump.ok_or_else(|| {
                        "the embedded Code Mode bootstrap is not waiting for input".to_owned()
                    })?;
                    if self.instance.resolve_unhandled(pump).map_err(failure)? > 0 {
                        self.pump = None;
                        continue;
                    }
                    if state.settling() {
                        self.instance
                            .resolve(pump, Ok(Value::Null))
                            .map_err(failure)?;
                        self.pump = None;
                        continue;
                    }
                    state.flush_text();
                    self.phase = Phase::Idle;
                    return Ok(Ran::Idle);
                }
                Step::Done => {
                    if let Replaying::Diverged(reason) = state.replaying(true) {
                        state.send(RuntimeEvent::Diverged {
                            cell_id: state.execution_id,
                            reason,
                        });
                        return Ok(Ran::Finished);
                    }
                    let encoded = self
                        .instance
                        .result()
                        .map_err(failure)?
                        .as_str()
                        .ok_or_else(|| {
                            "embedded Code Mode returned a result that is not text".to_owned()
                        })?
                        .into_owned();
                    state.send_terminal(&encoded)?;
                    return Ok(Ran::Finished);
                }
            }
        }
    }

    fn deliver(&mut self, value: Value<'_>, settled: Settled) -> Result<(), String> {
        let pump = self.pump.take().ok_or_else(|| {
            "the embedded Code Mode bootstrap is not waiting for input".to_owned()
        })?;
        self.instance.resolve(pump, Ok(value)).map_err(failure)?;
        self.phase = Phase::Running;
        self.settled = Some(settled);
        #[cfg(target_family = "wasm")]
        {
            self.slices = 0;
        }
        Ok(())
    }

    fn deliver_result(
        &mut self,
        state: &mut ExecutionState,
        id: u64,
        encoded: &str,
        success: bool,
    ) -> Result<(), String> {
        state.resolve_tool(id, encoded, success)?;
        self.deliver(
            Value::Object(&[
                ("id", Value::Number(id as f64)),
                ("ok", Value::Bool(success)),
                ("value", Value::Str(encoded)),
            ]),
            Settled::Waiting,
        )
    }

    fn deliver_timeout(&mut self, state: &mut ExecutionState, id: u64) -> Result<(), String> {
        if !state.fire_timeout(id) {
            return Ok(());
        }
        self.deliver(Value::Number(id as f64), Settled::Checkpoint)
    }

    fn idle(&mut self, state: &mut ExecutionState) -> Result<Idle, String> {
        match state.replaying(false) {
            Replaying::Live => {}
            Replaying::Diverged(reason) => {
                state.send(RuntimeEvent::Diverged {
                    cell_id: state.execution_id,
                    reason,
                });
                return Ok(Idle::Finished);
            }
            Replaying::Input(JournalEntry::Result {
                id,
                encoded,
                success,
            }) => {
                self.deliver_result(state, id, &encoded, success)?;
                self.settled = Some(Settled::Checkpoint);
                return Ok(Idle::Busy);
            }
            Replaying::Input(JournalEntry::Timer { id }) => {
                self.deliver_timeout(state, id)?;
                return Ok(Idle::Busy);
            }
            Replaying::Input(_) => return Ok(Idle::Busy),
        }
        if self.held {
            return Ok(Idle::Quiet(None));
        }
        if let Some((id, encoded, success)) = self.results.pop_front() {
            self.deliver_result(state, id, &encoded, success)?;
            return Ok(Idle::Busy);
        }
        match self.settled.take() {
            Some(Settled::Checkpoint) => state.send_checkpoint(),
            Some(Settled::Waiting) => state.send_waiting(),
            None => {}
        }
        if !state.events.is_empty() {
            return Ok(Idle::Busy);
        }
        Ok(Idle::Quiet(state.next_timeout_wait()))
    }
}

enum Idle {
    Busy,
    Finished,
    Quiet(Option<std::time::Duration>),
}

fn failure(error: djs_embed::Error) -> String {
    format!("embedded Code Mode execution failed: {error}")
}

pub struct EmbeddedHost {
    state: ExecutionState,
    machine: Option<Machine>,
    failure: Option<String>,
}

impl EmbeddedHost {
    pub fn spawn() -> Result<Self, String> {
        Ok(Self {
            state: ExecutionState::new(),
            machine: None,
            failure: None,
        })
    }

    pub async fn spawn_available() -> Result<Self, String> {
        Self::spawn()
    }

    pub fn start_cell(
        &mut self,
        execution_id: u64,
        source: &str,
        stored: HashMap<String, Arc<RawValue>>,
        tools: Arc<CellTools>,
        prelude: Option<Arc<str>>,
        journaling: Option<CellJournaling>,
    ) -> Result<(), String> {
        self.machine = None;
        let snapshot = snapshot(&tools, prelude.as_ref());
        self.state.begin(execution_id, stored, tools, journaling);
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(Unprepared::Prelude(message)) => {
                self.state.send(RuntimeEvent::Error {
                    cell_id: execution_id,
                    message,
                    stored: HashMap::new(),
                });
                return Ok(());
            }
            Err(Unprepared::Bootstrap(message)) => return Err(message),
        };
        let seed = random_seed()?;
        let mut instance = snapshot
            .instantiate(Limits {
                memory: usize::MAX,
                fuel: u64::MAX,
                ..Limits::default()
            })
            .map_err(failure)?;
        instance.seed_random(seed).map_err(failure)?;
        #[cfg(not(target_family = "wasm"))]
        instance
            .set_time_zone_offset(chrono::Local::now().offset().local_minus_utc() / 60)
            .map_err(failure)?;
        instance
            .start_export(BOOTSTRAP_MODULE, RUN_CELL, &[Value::Str(source)])
            .map_err(failure)?;
        self.machine = Some(Machine {
            instance,
            pump: None,
            phase: Phase::Running,
            results: VecDeque::new(),
            held: false,
            settled: Some(Settled::Checkpoint),
            #[cfg(target_family = "wasm")]
            slices: 0,
        });
        Ok(())
    }

    pub async fn read_event(&mut self) -> Result<RuntimeEvent, String> {
        loop {
            if let Some(event) = self.state.events.pop_front() {
                return Ok(event);
            }
            if let Some(failure) = self.failure.take() {
                return Err(failure);
            }
            let Some(machine) = self.machine.as_mut() else {
                return std::future::pending().await;
            };
            let quiet = match machine.phase {
                Phase::Running => match machine.run(&mut self.state) {
                    Ok(Ran::Sliced) => {
                        self.sliced().await;
                        continue;
                    }
                    Ok(Ran::Idle) => continue,
                    Ok(Ran::Finished) => Ok(Idle::Finished),
                    Err(error) => Err(error),
                },
                Phase::Idle => machine.idle(&mut self.state),
            };
            match quiet {
                Ok(Idle::Busy) => {}
                Ok(Idle::Finished) => self.stop(),
                Ok(Idle::Quiet(None)) => return std::future::pending().await,
                Ok(Idle::Quiet(Some(wait))) => {
                    nanocode_platform::sleep(wait).await;
                    if let Some(machine) = self.machine.as_mut()
                        && let Some(id) = self.state.next_due_timeout(Instant::now())
                        && let Err(error) = machine.deliver_timeout(&mut self.state, id)
                    {
                        self.fail(error);
                    }
                }
                Err(error) => self.fail(error),
            }
        }
    }

    #[cfg(not(target_family = "wasm"))]
    async fn sliced(&mut self) {
        tokio::task::yield_now().await;
    }

    #[cfg(target_family = "wasm")]
    async fn sliced(&mut self) {
        if self.machine.as_mut().is_some_and(|machine| {
            machine.slices += 1;
            machine.slices >= INTERRUPT_CHECKS.get()
        }) {
            self.state.send(RuntimeEvent::Error {
                cell_id: self.state.execution_id,
                message: EXHAUSTED.to_owned(),
                stored: HashMap::new(),
            });
            self.stop();
        }
    }

    fn stop(&mut self) {
        self.machine = None;
        self.state.clear();
    }

    fn fail(&mut self, error: String) {
        tracing::error!(
            target: "nanocode_code_mode",
            %error,
            "an embedded Code Mode cell stopped"
        );
        self.stop();
        self.failure = Some(error);
    }

    pub fn send_tool_result(
        &mut self,
        execution_id: u64,
        id: u64,
        encoded: String,
        success: bool,
    ) -> Result<(), String> {
        if self.state.execution_id == execution_id
            && let Some(machine) = self.machine.as_mut()
        {
            machine.results.push_back((id, encoded, success));
        }
        Ok(())
    }

    pub fn hold(&mut self, execution_id: u64) -> Result<(), String> {
        if self.state.execution_id == execution_id
            && let Some(machine) = self.machine.as_mut()
            && !machine.held
        {
            machine.held = true;
            self.state.send(RuntimeEvent::Held {
                cell_id: execution_id,
            });
        }
        Ok(())
    }

    pub const fn release(&mut self, execution_id: u64) -> Result<(), String> {
        if self.state.execution_id == execution_id
            && let Some(machine) = self.machine.as_mut()
            && machine.held
        {
            machine.held = false;
            machine.settled = Some(Settled::Waiting);
        }
        Ok(())
    }

    pub async fn terminate(&mut self) {
        self.stop();
    }
}
