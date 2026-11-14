use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use djs_embed::{CallId, Instance, Interrupt, Parked, Step, Value};
use nanocode_platform::Instant;
use serde_json::value::RawValue;

use crate::{
    cell::{
        BOOTSTRAP_MODULE, CellTools, ExecutionState, NEXT, Prepared, RUN_CELL, Replaying, Reply,
        RuntimeEvent, Unprepared, prepared,
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

fn random_seed() -> Result<u64, String> {
    let mut seed = [0; size_of::<u64>()];
    nanocode_platform::fill_random(&mut seed)
        .map_err(|error| format!("embedded Code Mode has no randomness: {error}"))?;
    Ok(u64::from_le_bytes(seed))
}

#[cfg(not(target_family = "wasm"))]
const SECONDS_PER_MINUTE: i32 = 60;
const PARK_AFTER: Duration = Duration::from_millis(1);

#[derive(Clone, Copy)]
enum Phase {
    Running,
    Idle(Instant),
}

enum Body {
    Awake(Instance),
    Parked(Parked),
}

fn parked() -> String {
    "the embedded Code Mode cell is parked".to_owned()
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
    Interrupted,
}

struct Machine {
    body: Body,
    prepared: Arc<Prepared>,
    interrupt: Interrupt,
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
        let Body::Awake(instance) = &mut self.body else {
            return Err(parked());
        };
        let slice = instance.limits().slice;
        let started = instance.fuel_used();
        loop {
            let step = match instance.step() {
                Ok(step) => step,
                Err(djs_embed::Error::OutOfMemory) => {
                    state.send(RuntimeEvent::Error {
                        cell_id: state.execution_id,
                        message: "Script stopped: the cell ran out of memory.".to_owned(),
                        stored: HashMap::new(),
                    });
                    return Ok(Ran::Finished);
                }
                Err(djs_embed::Error::Interrupted) => return Ok(Ran::Interrupted),
                Err(error) => return Err(failure(error)),
            };
            match step {
                Step::Yield => return Ok(Ran::Sliced),
                Step::HostCall(id) => {
                    let call = instance.host_call(id).map_err(failure)?;
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
                    if instance.fuel_used().saturating_sub(started) >= slice {
                        return Ok(Ran::Sliced);
                    }
                }
                Step::Wait(started) => {
                    if !started.is_empty() {
                        for id in started {
                            if instance.host_call(id).map_err(failure)?.fn_id() == NEXT {
                                self.pump = Some(id);
                            }
                        }
                        continue;
                    }
                    let pump = self.pump.ok_or_else(|| {
                        "the embedded Code Mode bootstrap is not waiting for input".to_owned()
                    })?;
                    if instance.resolve_unhandled(pump).map_err(failure)? > 0 {
                        self.pump = None;
                        continue;
                    }
                    if state.settling() {
                        instance.resolve(pump, Ok(Value::Null)).map_err(failure)?;
                        self.pump = None;
                        continue;
                    }
                    state.flush_text();
                    self.phase = Phase::Idle(nanocode_platform::now());
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
                    let encoded = instance
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
        let Body::Awake(instance) = &mut self.body else {
            return Err(parked());
        };
        let pump = self.pump.take().ok_or_else(|| {
            "the embedded Code Mode bootstrap is not waiting for input".to_owned()
        })?;
        instance.resolve(pump, Ok(value)).map_err(failure)?;
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
        if matches!(self.body, Body::Parked(_)) && !self.held && !self.results.is_empty() {
            return Ok(Idle::Wake);
        }
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
        let parks = self.parks();
        if self.held {
            return Ok(if parks {
                Idle::Park
            } else {
                Idle::Quiet {
                    timer: None,
                    park: None,
                }
            });
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
        let timer = state.next_timeout_wait();
        if parks && !state.awaits_tools() && timer.is_none_or(|timer| timer >= PARK_AFTER) {
            return Ok(Idle::Park);
        }
        Ok(Idle::Quiet {
            timer,
            park: self
                .park_due()
                .map(|due| due.saturating_duration_since(nanocode_platform::now())),
        })
    }

    fn parks(&self) -> bool {
        match &self.body {
            Body::Awake(instance) => {
                let image = instance.snapshot().image_len();
                instance.heap_used().saturating_sub(image) < image
            }
            Body::Parked(_) => false,
        }
    }

    fn park_due(&self) -> Option<Instant> {
        match self.phase {
            Phase::Idle(since) if self.parks() => since.checked_add(PARK_AFTER),
            Phase::Idle(_) | Phase::Running => None,
        }
    }
}

enum Idle {
    Busy,
    Finished,
    Wake,
    Park,
    Quiet {
        timer: Option<Duration>,
        park: Option<Duration>,
    },
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

    #[allow(clippy::too_many_arguments)]
    pub fn start_cell(
        &mut self,
        execution_id: u64,
        source: &str,
        stored: HashMap<String, Arc<RawValue>>,
        tools: Arc<CellTools>,
        prelude: Option<Arc<str>>,
        journaling: Option<CellJournaling>,
        interrupt: Interrupt,
    ) -> Result<(), String> {
        self.machine = None;
        let prepared = prepared(&tools, prelude.as_ref());
        self.state.begin(execution_id, stored, tools, journaling);
        let prepared = match prepared {
            Ok(prepared) => prepared,
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
        let mut instance = prepared.instance().map_err(failure)?;
        instance.set_interrupt(Some(&interrupt)).map_err(failure)?;
        instance.seed_random(seed).map_err(failure)?;
        #[cfg(not(target_family = "wasm"))]
        instance
            .set_time_zone_offset(
                nanocode_platform::utc_offset_seconds(nanocode_platform::system_time())
                    / SECONDS_PER_MINUTE,
            )
            .map_err(failure)?;
        instance
            .start_export(BOOTSTRAP_MODULE, RUN_CELL, &[Value::Str(source)])
            .map_err(failure)?;
        self.machine = Some(Machine {
            body: Body::Awake(instance),
            prepared,
            interrupt,
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
                    Ok(Ran::Interrupted) => {
                        self.stop();
                        continue;
                    }
                    Ok(Ran::Finished) => Ok(Idle::Finished),
                    Err(error) => Err(error),
                },
                Phase::Idle(_) => machine.idle(&mut self.state),
            };
            let rested = match quiet {
                Ok(Idle::Busy) => Ok(()),
                Ok(Idle::Finished) => {
                    self.finish();
                    Ok(())
                }
                Ok(Idle::Wake) => self.wake(),
                Ok(Idle::Park) => self.park(),
                Ok(Idle::Quiet { timer, park }) => {
                    let wait = match (timer, park) {
                        (None, None) => return std::future::pending().await,
                        (Some(wait), None) | (None, Some(wait)) => wait,
                        (Some(timer), Some(park)) => timer.min(park),
                    };
                    nanocode_platform::sleep(wait).await;
                    self.rested()
                }
                Err(error) => Err(error),
            };
            if let Err(error) = rested {
                self.fail(error);
            }
        }
    }

    fn rested(&mut self) -> Result<(), String> {
        let now = nanocode_platform::now();
        if let Some(id) = self.state.next_due_timeout(now) {
            self.wake()?;
            return match self.machine.as_mut() {
                Some(machine) => machine.deliver_timeout(&mut self.state, id),
                None => Ok(()),
            };
        }
        let due = self.machine.as_ref().and_then(Machine::park_due);
        if due.is_some_and(|due| due <= now) {
            self.park()?;
        }
        Ok(())
    }

    fn park(&mut self) -> Result<(), String> {
        let Some(machine) = self.machine.take() else {
            return Ok(());
        };
        let body = match machine.body {
            Body::Awake(instance) => {
                let (parked, spare) = instance.park().map_err(failure)?;
                if let Some(spare) = spare {
                    machine.prepared.keep(spare);
                }
                Body::Parked(parked)
            }
            body @ Body::Parked(_) => body,
        };
        self.machine = Some(Machine { body, ..machine });
        Ok(())
    }

    fn wake(&mut self) -> Result<(), String> {
        let Some(machine) = self.machine.take() else {
            return Ok(());
        };
        let body = match machine.body {
            Body::Parked(parked) => {
                let mut instance = match machine.prepared.spare() {
                    Some(spare) => parked.resume_in(spare),
                    None => parked.resume(),
                }
                .map_err(failure)?;
                instance
                    .set_interrupt(Some(&machine.interrupt))
                    .map_err(failure)?;
                Body::Awake(instance)
            }
            body @ Body::Awake(_) => body,
        };
        self.machine = Some(Machine { body, ..machine });
        Ok(())
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

    fn finish(&mut self) {
        if let Some(Machine {
            body: Body::Awake(instance),
            prepared,
            ..
        }) = self.machine.take()
        {
            prepared.recycle(instance);
        }
        self.state.clear();
    }

    fn fail(&mut self, error: String) {
        nanocode_trace::error!(
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
