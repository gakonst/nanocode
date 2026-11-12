use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures_util::future::{Either, select};
use nanocode_agent_core::{
    Agent, AgentHandle, AgentSessionContext, Capabilities, ClaudeModel, ForkRequest, HarnessFamily,
    Lineage, ModelSelection, NanocodeError, Result, SessionCheckpoint, SpawnOptions, Thinking,
    TurnResult,
    backend::{
        AgentFactory, BackendFuture, BackendPrompt, BackendPromptRoute, BackendRuntime,
        BackendTurn, BackendTurnKey, LifecycleBackend, ParkedCall,
    },
    events::{AgentEventKind, AgentEventPublisher, AgentEvents},
};
use nanocode_subagents::{MemorySubagentStore, SubagentStore, SubagentStoreFuture};
use nanocode_sync::{Atom, Signal, Slot};
use serde_json::{Value, value::RawValue};
use tokio::sync::Notify;

use crate::{
    executor::Clock,
    rng::{FNV_OFFSET, Rng, fnv},
};

const RUNNING: u8 = 0;
const PARKED: u8 = 1;
const CLAIMED: u8 = 2;
const STEP_EVENT_PAYLOAD: &str = "{}";

#[derive(Clone, Copy, Debug)]
pub struct Script {
    pub steps_max: u64,
    pub step_latency_ms_min: u64,
    pub step_latency_ms_max: u64,
    pub restore_latency_ms_max: u64,
    pub shutdown_latency_ms_max: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub spawned: u64,
    pub restored: u64,
    pub parked: u64,
    pub steps: u64,
    pub turns: u64,
}

pub struct Ledger {
    script: Script,
    clock: Arc<Clock>,
    random: Slot<Rng>,
    digest: AtomicU64,
    next_session: AtomicU64,
    open_agents: AtomicUsize,
    running_turns: AtomicUsize,
    steps_left: Atom<BTreeMap<String, u64>>,
    pending: Atom<BTreeMap<String, (String, Value)>>,
    counts: Atom<Counts>,
}

impl Ledger {
    pub fn new(seed: u64, script: Script, clock: Arc<Clock>) -> Arc<Self> {
        Arc::new(Self {
            script,
            clock,
            random: Slot::new(Some(Rng::derive(seed, "agents"))),
            digest: AtomicU64::new(FNV_OFFSET),
            next_session: AtomicU64::new(0),
            open_agents: AtomicUsize::new(0),
            running_turns: AtomicUsize::new(0),
            steps_left: Atom::default(),
            pending: Atom::default(),
            counts: Atom::default(),
        })
    }

    pub fn trace(&self, event: &str) {
        let digest = fnv(
            self.digest.load(Ordering::SeqCst),
            &self.clock.now_ns().to_le_bytes(),
        );
        self.digest
            .store(fnv(digest, event.as_bytes()), Ordering::SeqCst);
    }

    pub fn digest(&self) -> u64 {
        self.digest.load(Ordering::SeqCst)
    }

    pub fn open_agents(&self) -> usize {
        self.open_agents.load(Ordering::SeqCst)
    }

    pub fn running_turns(&self) -> usize {
        self.running_turns.load(Ordering::SeqCst)
    }

    pub fn counts(&self) -> Counts {
        *self.counts.get()
    }

    fn count(&self, change: impl Fn(&mut Counts)) {
        self.counts.update(|counts| change(counts));
    }

    fn range(&self, low: u64, high: u64) -> u64 {
        let mut random = self
            .random
            .take()
            .expect("the simulator polls one task at a time, so the script's randomness is home");
        let drawn = random.range(low, high);
        assert!(
            self.random.put(random).is_none(),
            "the script's randomness has one home"
        );
        drawn
    }

    fn begin_turn(&self, session_id: &str, request_id: &str, input: &Value) {
        if !self.steps_left.read(|left| left.contains_key(session_id)) {
            let steps = self.range(1, self.script.steps_max);
            self.steps_left
                .update(|left| left.insert(session_id.to_owned(), steps));
        }
        self.pending.update(|pending| {
            pending.insert(
                session_id.to_owned(),
                (request_id.to_owned(), input.clone()),
            )
        });
        self.running_turns.fetch_add(1, Ordering::SeqCst);
        self.count(|counts| counts.turns += 1);
        self.trace(&format!("turn {session_id} {request_id}"));
    }

    fn steps_left(&self, session_id: &str) -> u64 {
        self.steps_left
            .read(|left| left.get(session_id).copied())
            .expect("a turn's steps are drawn when it begins")
    }

    fn step_done(&self, session_id: &str) {
        self.steps_left.update(|left| {
            let steps = left.get_mut(session_id).expect("a stepping turn has steps");
            assert!(*steps > 0, "a turn steps only while steps are left");
            *steps -= 1;
        });
        self.count(|counts| counts.steps += 1);
        self.trace(&format!("step {session_id}"));
    }

    fn finish_turn(&self, session_id: &str) {
        self.pending.update(|pending| pending.remove(session_id));
        self.trace(&format!("finished {session_id}"));
    }

    pub fn pending_turn(&self, session_id: &str) -> Option<(String, Value)> {
        self.pending
            .read(|pending| pending.get(session_id).cloned())
    }

    fn agent(self: &Arc<Self>, session_id: &str, model: ModelSelection) -> (Agent, AgentEvents) {
        let (runtime, events) = BackendRuntime::new(session_id.to_owned());
        let backend = ScriptedBackend {
            inner: Arc::new(Inner {
                session_id: Arc::from(session_id),
                ledger: Arc::clone(self),
                model,
                parking: AtomicBool::new(false),
                phase: AtomicU8::new(RUNNING),
                entries: AtomicU64::new(0),
                entered: Notify::new(),
                flight: Slot::default(),
                adopted: Slot::default(),
                stopped: Arc::new(Signal::new()),
                shut: AtomicBool::new(false),
                cancel: Atom::default(),
            }),
        };
        self.open_agents.fetch_add(1, Ordering::SeqCst);
        self.trace(&format!("agent {session_id}"));
        (runtime.bind(backend), events)
    }
}

pub fn simulated_model() -> ModelSelection {
    ModelSelection::Claude(ClaudeModel::Sonnet55)
}

struct TurnRunning(Arc<Ledger>);

impl Drop for TurnRunning {
    fn drop(&mut self) {
        self.0.running_turns.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Flight {
    done: Arc<Signal>,
}

impl Flight {
    fn start(ledger: &Ledger) -> Arc<Self> {
        let done = Arc::new(Signal::new());
        let latency = Duration::from_millis(ledger.range(
            ledger.script.step_latency_ms_min,
            ledger.script.step_latency_ms_max,
        ));
        let landing = Arc::clone(&done);
        nanocode_platform::spawn(async move {
            nanocode_platform::sleep(latency).await;
            landing.fire();
        });
        Arc::new(Self { done })
    }
}

impl ParkedCall for Flight {
    fn finished(&self) -> BackendFuture<()> {
        let done = Arc::clone(&self.done);
        Box::pin(async move { done.wait().await })
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync> {
        self
    }
}

struct Inner {
    session_id: Arc<str>,
    ledger: Arc<Ledger>,
    model: ModelSelection,
    parking: AtomicBool,
    phase: AtomicU8,
    entries: AtomicU64,
    entered: Notify,
    flight: Slot<Arc<Flight>>,
    adopted: Slot<Arc<Flight>>,
    stopped: Arc<Signal>,
    shut: AtomicBool,
    cancel: Atom<Option<Arc<Signal>>>,
}

enum Step {
    Landed,
    Stopped,
    Cancelled,
}

impl Inner {
    fn enter(&self, flight: &Arc<Flight>) {
        self.flight.put(Arc::clone(flight));
        self.phase.store(PARKED, Ordering::SeqCst);
        self.entries.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_waiters();
    }

    fn leave(&self) -> bool {
        if self
            .phase
            .compare_exchange(PARKED, RUNNING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            drop(self.flight.take());
        }
        self.phase.load(Ordering::SeqCst) != CLAIMED
    }

    async fn step(&self, cancel: &Signal) -> Step {
        let flight = self
            .adopted
            .take()
            .unwrap_or_else(|| Flight::start(&self.ledger));
        let parks = self.parking.load(Ordering::SeqCst);
        if parks {
            self.enter(&flight);
        }
        let landed = Box::pin(flight.done.wait());
        let stopped = Box::pin(self.stopped.wait());
        let cancelled = Box::pin(cancel.wait());
        let step = match select(landed, select(stopped, cancelled)).await {
            Either::Left(_) => Step::Landed,
            Either::Right((Either::Left(_), _)) => Step::Stopped,
            Either::Right((Either::Right(_), _)) => Step::Cancelled,
        };
        if parks && !self.leave() {
            return Step::Stopped;
        }
        step
    }

    async fn run(
        self: Arc<Self>,
        events: AgentEventPublisher,
        request_id: String,
        cancel: Arc<Signal>,
        running: TurnRunning,
    ) -> Result<TurnResult> {
        while self.ledger.steps_left(&self.session_id) > 0 {
            let payload: Arc<RawValue> = RawValue::from_string(STEP_EVENT_PAYLOAD.to_owned())
                .expect("the step event's payload is JSON")
                .into();
            drop(events.publish_next(AgentEventKind::ModelCallStarted, payload));
            match self.step(&cancel).await {
                Step::Landed => self.ledger.step_done(&self.session_id),
                Step::Stopped => {
                    self.ledger.trace(&format!("stopped {}", self.session_id));
                    return Err(NanocodeError::TurnStopped);
                }
                Step::Cancelled => {
                    self.ledger.trace(&format!("cancelled {}", self.session_id));
                    return Err(NanocodeError::TurnCancelled);
                }
            }
        }
        self.ledger.finish_turn(&self.session_id);
        drop(running);
        Ok(TurnResult {
            request_id: Some(request_id),
            turn_id: None,
            final_message: String::new(),
            usage: None,
            boundary: None,
        })
    }

    fn checkpoint(&self) -> SessionCheckpoint {
        SessionCheckpoint::native(
            self.session_id.to_string(),
            self.model.clone(),
            self.model.default_thinking(),
            Lineage::root(self.session_id.to_string()),
            self.session_id.to_string(),
            None,
            true,
            Value::Object(serde_json::Map::new()),
        )
    }
}

pub struct ScriptedBackend {
    inner: Arc<Inner>,
}

fn unsupported<T: Send + 'static>(capability: &'static str) -> BackendFuture<Result<T>> {
    Box::pin(async move { Err(NanocodeError::UnsupportedCapability { capability }) })
}

fn done() -> BackendFuture<Result<()>> {
    Box::pin(async { Ok(()) })
}

impl ScriptedBackend {
    fn start(&self, prompt: BackendPrompt) -> Result<BackendTurn> {
        let inner = Arc::clone(&self.inner);
        let request_id = prompt
            .request_id
            .clone()
            .unwrap_or_else(|| format!("{}-{}", inner.session_id, prompt.key.0));
        let input = serde_json::to_value(&prompt.prompt)
            .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?;
        inner
            .ledger
            .begin_turn(&inner.session_id, &request_id, &input);
        let running = TurnRunning(Arc::clone(&inner.ledger));
        let cancel = Arc::new(Signal::new());
        inner.cancel.set(Some(Arc::clone(&cancel)));
        let result = Box::pin(inner.run(prompt.events, request_id.clone(), cancel, running));
        Ok(BackendTurn {
            request_id: Some(request_id),
            result,
        })
    }

    fn cancel_turn(&self) -> BackendFuture<Result<()>> {
        if let Some(cancel) = Option::clone(&self.inner.cancel.get()) {
            cancel.fire();
        }
        done()
    }
}

impl LifecycleBackend for ScriptedBackend {
    fn harness_family(&self) -> HarnessFamily {
        HarnessFamily::Claude
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::NONE;
        capabilities.checkpoint = true;
        capabilities.resume = true;
        capabilities
    }

    fn submit(&self, prompt: BackendPrompt) -> BackendFuture<Result<BackendTurn>> {
        let started = self.start(prompt);
        Box::pin(async move { started })
    }

    fn route(&self, prompt: BackendPrompt) -> BackendFuture<Result<BackendPromptRoute>> {
        let started = self.start(prompt).map(BackendPromptRoute::Started);
        Box::pin(async move { started })
    }

    fn steer(
        &self,
        _key: BackendTurnKey,
        _prompt: nanocode_agent_core::input::Prompt,
    ) -> BackendFuture<Result<()>> {
        unsupported("steering")
    }

    fn cancel(&self, _key: BackendTurnKey) -> BackendFuture<Result<()>> {
        self.cancel_turn()
    }

    fn cancel_active(&self) -> BackendFuture<Result<()>> {
        self.cancel_turn()
    }

    fn set_harness_model(&self, _model: ModelSelection) -> BackendFuture<Result<()>> {
        done()
    }

    fn set_thinking(&self, _thinking: Thinking) -> BackendFuture<Result<()>> {
        done()
    }

    fn set_fast_mode(&self, _enabled: bool) -> BackendFuture<Result<()>> {
        done()
    }

    fn compact(&self) -> BackendFuture<Result<()>> {
        unsupported("compaction")
    }

    fn append_developer_message(
        &self,
        _text: String,
    ) -> BackendFuture<Result<AgentSessionContext>> {
        unsupported("developer_messages")
    }

    fn context(&self) -> BackendFuture<Result<AgentSessionContext>> {
        unsupported("context")
    }

    fn checkpoint(&self) -> BackendFuture<Result<SessionCheckpoint>> {
        let checkpoint = self.inner.checkpoint();
        Box::pin(async move { Ok(checkpoint) })
    }

    fn spawn(&self, _options: SpawnOptions) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        unsupported("spawn")
    }

    fn fork(&self, _request: ForkRequest) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        unsupported("fork")
    }

    fn flush(&self) -> BackendFuture<Result<()>> {
        done()
    }

    fn shutdown(&self) -> BackendFuture<Result<()>> {
        let inner = &self.inner;
        inner.stopped.fire();
        if inner.shut.swap(true, Ordering::SeqCst) {
            return done();
        }
        let ledger = &inner.ledger;
        ledger.open_agents.fetch_sub(1, Ordering::SeqCst);
        ledger.trace(&format!("shutdown {}", inner.session_id));
        let latency = Duration::from_millis(ledger.range(0, ledger.script.shutdown_latency_ms_max));
        Box::pin(async move {
            nanocode_platform::sleep(latency).await;
            Ok(())
        })
    }

    fn set_parking(&self, parking: bool) {
        self.inner.parking.store(parking, Ordering::SeqCst);
    }

    fn parked(&self, after: u64) -> BackendFuture<u64> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            loop {
                let entered = inner.entered.notified();
                tokio::pin!(entered);
                entered.as_mut().enable();
                let entries = inner.entries.load(Ordering::SeqCst);
                if entries > after && inner.phase.load(Ordering::SeqCst) == PARKED {
                    return entries;
                }
                entered.await;
            }
        })
    }

    fn park(&self) -> BackendFuture<Result<Option<Arc<dyn ParkedCall>>>> {
        let inner = &self.inner;
        let claimed = inner
            .phase
            .compare_exchange(PARKED, CLAIMED, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        let Some(flight) = claimed.then(|| inner.flight.take()).flatten() else {
            return Box::pin(async { Ok(None) });
        };
        inner.ledger.count(|counts| counts.parked += 1);
        inner.ledger.trace(&format!("park {}", inner.session_id));
        let stopped = self.shutdown();
        Box::pin(async move {
            stopped.await?;
            Ok(Some(flight as Arc<dyn ParkedCall>))
        })
    }

    fn unpark(&self, call: Arc<dyn ParkedCall>) {
        let flight = call
            .into_any()
            .downcast::<Flight>()
            .unwrap_or_else(|_| panic!("a scripted agent parks only its own model calls"));
        self.inner.adopted.put(flight);
        self.inner.parking.store(true, Ordering::SeqCst);
        self.inner
            .ledger
            .trace(&format!("unpark {}", self.inner.session_id));
    }

    fn is_stopped(&self) -> bool {
        self.inner.shut.load(Ordering::SeqCst)
    }
}

pub struct ScriptedFactory {
    ledger: Arc<Ledger>,
}

impl ScriptedFactory {
    pub const fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }
}

impl AgentFactory for ScriptedFactory {
    fn spawn(
        &self,
        parent: AgentHandle,
        _options: SpawnOptions,
        _host_context: Option<Arc<str>>,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        let ledger = Arc::clone(&self.ledger);
        Box::pin(async move {
            let number = ledger.next_session.fetch_add(1, Ordering::SeqCst);
            ledger.count(|counts| counts.spawned += 1);
            Ok(ledger.agent(&format!("child-{number}"), parent.harness_model()))
        })
    }

    fn restore(
        &self,
        _parent: AgentHandle,
        checkpoint: SessionCheckpoint,
        _host_context: Option<Arc<str>>,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        let ledger = Arc::clone(&self.ledger);
        let latency = Duration::from_millis(ledger.range(0, ledger.script.restore_latency_ms_max));
        ledger.trace(&format!("restoring {}", checkpoint.session_id()));
        Box::pin(async move {
            nanocode_platform::sleep(latency).await;
            ledger.count(|counts| counts.restored += 1);
            Ok(ledger.agent(checkpoint.session_id(), checkpoint.model()))
        })
    }
}

pub struct ScriptedStore {
    memory: MemorySubagentStore,
    ledger: Arc<Ledger>,
}

impl ScriptedStore {
    pub fn new(ledger: Arc<Ledger>) -> Self {
        Self {
            memory: MemorySubagentStore::new(),
            ledger,
        }
    }
}

impl SubagentStore for ScriptedStore {
    fn pending_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<Option<(String, Value)>>> {
        let pending = self.ledger.pending_turn(session_id);
        Box::pin(async move { Ok(pending) })
    }

    fn load<'a>(
        &'a self,
        root_session_id: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<Option<String>>> {
        self.memory.load(root_session_id)
    }

    fn save<'a>(
        &'a self,
        root_session_id: &'a str,
        payload: String,
        records: Vec<(Arc<str>, Arc<str>)>,
    ) -> SubagentStoreFuture<'a, std::io::Result<()>> {
        self.memory.save(root_session_id, payload, records)
    }

    fn record_session<'a>(
        &'a self,
        root_session_id: &'a str,
        checkpoint: SessionCheckpoint,
    ) -> SubagentStoreFuture<'a, std::io::Result<()>> {
        self.memory.record_session(root_session_id, checkpoint)
    }

    fn records_sessions(&self) -> bool {
        self.memory.records_sessions()
    }

    fn load_record<'a>(
        &'a self,
        root_session_id: &'a str,
        key: &'a str,
    ) -> SubagentStoreFuture<'a, std::io::Result<String>> {
        self.memory.load_record(root_session_id, key)
    }

    fn load_records<'a>(
        &'a self,
        root_session_id: &'a str,
        keys: Vec<String>,
    ) -> SubagentStoreFuture<'a, std::io::Result<Vec<String>>> {
        self.memory.load_records(root_session_id, keys)
    }
}
