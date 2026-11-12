use std::{cell::RefCell, sync::Arc, time::Duration};

use nanocode_agent_core::AgentHandle;
use nanocode_subagents::{
    AgentStatus, AgentTask, DEFAULT_MAX_SUBAGENTS, Registry, ScopedAgentUpdate, SubagentControl,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    executor::{Clock, Ended, Executor, Task},
    rng::Rng,
    scripted::{Counts, Ledger, Script, ScriptedFactory, ScriptedStore, simulated_model},
};

pub const CHILDREN_MIN: u64 = 1;
pub const CHILDREN_MAX: u64 = 64;
pub const RESIDENT_MAX: u64 = 4;
pub const TURN_STEPS_MAX: u64 = 8;
pub const STEP_LATENCY_MS_MIN: u64 = 50;
pub const STEP_LATENCY_MS_MAX: u64 = 20_000;
pub const RESTORE_LATENCY_MS_MAX: u64 = 2_000;
pub const SHUTDOWN_LATENCY_MS_MAX: u64 = 1_000;
pub const CANCEL_PPM: u64 = 250_000;
pub const RUNTIME_STOP_TIMEOUT: Duration = Duration::from_secs(30);
const ROOT_SESSION: &str = "root";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Close,
    Cancel,
}

struct Shape {
    children: u64,
    resident_max: usize,
    stop_after: Duration,
    stop: Stop,
    script: Script,
}

impl Shape {
    fn of(seed: u64) -> Self {
        let mut rng = Rng::derive(seed, "agents-shape");
        let children = rng.range(CHILDREN_MIN, CHILDREN_MAX);
        let resident_max = rng.range(0, RESIDENT_MAX);
        let stop = if rng.chance(CANCEL_PPM) {
            Stop::Cancel
        } else {
            Stop::Close
        };
        let script = Script {
            steps_max: rng.range(1, TURN_STEPS_MAX),
            step_latency_ms_min: STEP_LATENCY_MS_MIN,
            step_latency_ms_max: rng.range(STEP_LATENCY_MS_MIN, STEP_LATENCY_MS_MAX),
            restore_latency_ms_max: rng.range(0, RESTORE_LATENCY_MS_MAX),
            shutdown_latency_ms_max: rng.range(0, SHUTDOWN_LATENCY_MS_MAX),
        };
        let turn_ms_max = script.steps_max * script.step_latency_ms_max;
        let stop_after = Duration::from_millis(rng.range(0, turn_ms_max));
        Self {
            children,
            resident_max: usize::try_from(resident_max).expect("a resident bound fits in usize"),
            stop_after,
            stop,
            script,
        }
    }

    fn describe(&self) -> String {
        format!(
            "scenario=agents children={} resident<={} stop={:?} after_ms={} steps<={} latency_ms<={} restore_ms<={} shutdown_ms<={}",
            self.children,
            self.resident_max,
            self.stop,
            self.stop_after.as_millis(),
            self.script.steps_max,
            self.script.step_latency_ms_max,
            self.script.restore_latency_ms_max,
            self.script.shutdown_latency_ms_max
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AgentStats {
    pub seeds: u64,
    pub closes: u64,
    pub cancels: u64,
    pub children: u64,
    pub spawned: u64,
    pub restored: u64,
    pub parked: u64,
    pub steps: u64,
    pub turns: u64,
    pub stop_ms_max: u64,
}

impl AgentStats {
    pub fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.closes += other.closes;
        self.cancels += other.cancels;
        self.children += other.children;
        self.spawned += other.spawned;
        self.restored += other.restored;
        self.parked += other.parked;
        self.steps += other.steps;
        self.turns += other.turns;
        self.stop_ms_max = self.stop_ms_max.max(other.stop_ms_max);
    }

    fn of(shape: &Shape, counts: Counts, stop_ms: u64) -> Self {
        Self {
            seeds: 1,
            closes: u64::from(shape.stop == Stop::Close),
            cancels: u64::from(shape.stop == Stop::Cancel),
            children: shape.children,
            spawned: counts.spawned,
            restored: counts.restored,
            parked: counts.parked,
            steps: counts.steps,
            turns: counts.turns,
            stop_ms_max: stop_ms,
        }
    }
}

pub struct AgentOutcome {
    pub digest: u64,
    pub wrong: Option<String>,
    pub stats: AgentStats,
    pub shape: String,
}

struct Run {
    registry: Arc<Registry>,
    control: SubagentControl,
    root: AgentHandle,
    updates: UnboundedReceiver<ScopedAgentUpdate>,
}

fn build(ledger: &Arc<Ledger>, shape: &Shape) -> Run {
    let (registry, control, updates) = nanocode_subagents::channel(DEFAULT_MAX_SUBAGENTS);
    control.set_hibernation(true);
    control.set_max_resident(shape.resident_max);
    registry.set_store(Arc::new(ScriptedStore::new(Arc::clone(ledger))));
    let root = AgentHandle::new(
        ROOT_SESSION,
        simulated_model(),
        Arc::new(ScriptedFactory::new(Arc::clone(ledger))),
    );
    Run {
        registry,
        control,
        root,
        updates,
    }
}

fn tasks(children: u64) -> Vec<AgentTask> {
    (0..children)
        .map(|child| AgentTask {
            role: "worker".to_owned(),
            task: format!("task {child}"),
            output_schema: Value::Object(serde_json::Map::new()),
        })
        .collect()
}

async fn stop(run: &Run, shape: &Shape, clock: &Clock) -> Result<u64, String> {
    let began_ns = clock.now_ns();
    match shape.stop {
        Stop::Close => run
            .control
            .close_all(ROOT_SESSION)
            .await
            .map_err(|error| format!("close_all failed: {error}"))?,
        Stop::Cancel => run.control.cancel_all(ROOT_SESSION).await,
    }
    let took = Duration::from_nanos(clock.now_ns() - began_ns);
    let took_ms = u64::try_from(took.as_millis()).expect("a virtual stop fits in u64 milliseconds");
    if took >= RUNTIME_STOP_TIMEOUT {
        return Err(format!(
            "{:?} took {took_ms} ms of virtual time, not under the runtime's stop deadline of {} ms",
            shape.stop,
            RUNTIME_STOP_TIMEOUT.as_millis()
        ));
    }
    Ok(took_ms)
}

const fn settled(stop: Stop, status: &AgentStatus) -> bool {
    match stop {
        Stop::Close => matches!(status, AgentStatus::Closed),
        Stop::Cancel => matches!(
            status,
            AgentStatus::Interrupted | AgentStatus::Completed { .. } | AgentStatus::Failed { .. }
        ),
    }
}

async fn judge(run: &Run, shape: &Shape, ledger: &Ledger) -> Result<(), String> {
    let turns = ledger.running_turns();
    if turns > 0 {
        return Err(format!(
            "{turns} children still hold a turn after {:?}",
            shape.stop
        ));
    }
    let open = ledger.open_agents();
    if shape.stop == Stop::Close && open > 0 {
        return Err(format!(
            "{open} children still hold a resident harness after Close"
        ));
    }
    let listed = run
        .registry
        .directory(ROOT_SESSION, true, false)
        .await
        .map_err(|error| {
            format!(
                "the directory did not answer after {:?}: {error}",
                shape.stop
            )
        })?;
    let count = u64::try_from(listed.len()).expect("a child count fits in u64");
    if count != shape.children {
        return Err(format!(
            "the directory lists {count} of {} children",
            shape.children
        ));
    }
    for entry in listed {
        if !settled(shape.stop, &entry.status) {
            return Err(format!(
                "agent {} is {:?} after {:?}",
                entry.agent_id, entry.status, shape.stop
            ));
        }
    }
    Ok(())
}

async fn drive(shape: &Shape, ledger: &Arc<Ledger>, clock: &Clock) -> Result<u64, String> {
    let run = build(ledger, shape);
    nanocode_subagents::start_agents(
        &run.root,
        &run.registry,
        ROOT_SESSION,
        tasks(shape.children),
    )
    .await
    .map_err(|error| format!("start_agents failed: {error}"))?;
    ledger.trace("started");
    nanocode_platform::sleep(shape.stop_after).await;
    ledger.trace("stop");
    let took_ms = stop(&run, shape, clock).await?;
    ledger.trace("stopped");
    nanocode_platform::sleep(RUNTIME_STOP_TIMEOUT).await;
    judge(&run, shape, ledger).await?;
    drop(run.updates);
    Ok(took_ms)
}

pub fn simulate(seed: u64) -> AgentOutcome {
    let shape = Shape::of(seed);
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let ledger = Ledger::new(seed, shape.script, Arc::clone(&clock));
    let result = RefCell::new(None);
    let mut executor = Executor::new(Arc::clone(&clock), Rng::derive(seed, "scheduler"));
    let task: Task<'_> = Box::pin(async {
        let outcome = drive(&shape, &ledger, &clock).await;
        result.replace(Some(outcome));
    });
    let ended = executor.run(vec![task], None);
    let outcome = match (ended, result.into_inner()) {
        (_, Some(outcome)) => outcome,
        (Ended::Stuck, None) => Err("every task waits and no timer is set".to_owned()),
        (ended, None) => Err(format!("the run ended {ended:?} without a verdict")),
    };
    let stop_ms = *outcome.as_ref().unwrap_or(&0);
    AgentOutcome {
        digest: ledger.digest(),
        wrong: outcome.err(),
        stats: AgentStats::of(&shape, ledger.counts(), stop_ms),
        shape: shape.describe(),
    }
}

pub fn run_seed(seed: u64) -> AgentOutcome {
    std::panic::catch_unwind(|| simulate(seed)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        AgentOutcome {
            digest: 0,
            wrong: Some(format!("panic: {message}")),
            stats: AgentStats::default(),
            shape: String::new(),
        }
    })
}
