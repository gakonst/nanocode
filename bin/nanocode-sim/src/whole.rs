use std::{cell::RefCell, collections::VecDeque, path::PathBuf, sync::Arc, time::Duration};

use nanocode::{
    ClaudeClient, Nanocode, PromptRequest, Thinking, Tools,
    durability::{MemoryStore, SessionStore, TurnStatus},
};
use nanocode_durability_core::KeyValueStore;
use serde_json::{Value, json};

use crate::{
    executor::{Clock, Ended, Executor, Task},
    faults::Profile,
    httpd::{Connector, HttpFaults, HttpServer, WireMutation},
    network::{Network, NetworkFaults, Transport},
    rng::Rng,
    server::{EFFECT_TOOL, final_text, root_prompt},
    store::{Mutation, SimKv},
    world::{World, WorldCounts},
};

pub const ROOTS_MAX: u64 = 2;
pub const TURNS_MAX: u64 = 3;
pub const CRASHES_MAX: u64 = 3;
pub const CRASH_HORIZON_MS: u64 = 120_000;
pub const STOP_AFTER_MS_MAX: u64 = 60_000;
pub const STOP_PPM: u64 = 200_000;
pub const HIBERNATION_PPM: u64 = 500_000;
pub const RESIDENT_MAX: u64 = 4;
pub const SETTLE_DEADLINE: Duration = Duration::from_secs(1_800);
pub const SETTLE_POLL: Duration = Duration::from_secs(1);
pub const RESUBMITS_MAX: u64 = 8;
pub const RESUBMIT_DELAY: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const ENDPOINT: &str = "http://model.sim/v1";
const MESSAGES_URL: &str = "http://model.sim/v1/messages";
const EPOCHS_MAX: u64 = CRASHES_MAX + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    Memory,
    Kv,
}

impl StoreKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "memory" => Some(Self::Memory),
            "kv" => Some(Self::Kv),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Kv => "kv",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WireKind {
    #[default]
    Transport,
    Http,
}

impl WireKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "transport" => Some(Self::Transport),
            "http" => Some(Self::Http),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::Http => "http",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WholeOptions {
    pub store: Option<StoreKind>,
    pub calm: bool,
    pub verbose: bool,
    pub wire: WireKind,
    pub mutation: WireMutation,
}

impl Default for WholeOptions {
    fn default() -> Self {
        Self {
            store: None,
            calm: false,
            verbose: false,
            wire: WireKind::Transport,
            mutation: WireMutation::None,
        }
    }
}

enum Wire {
    Transport(Arc<Network>),
    Http(Arc<HttpServer>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Never,
    Cancel,
    Close,
}

struct Shape {
    stop: Stop,
    stop_after: Duration,
    hibernation: Option<usize>,
    roots: u64,
    turns: u64,
    store: StoreKind,
    crashes: Vec<u64>,
    faults: NetworkFaults,
    http_faults: HttpFaults,
    wire: WireKind,
}

impl Shape {
    fn of(seed: u64, options: WholeOptions) -> Self {
        let mut rng = Rng::derive(seed, "whole-shape");
        let roots = rng.range(1, ROOTS_MAX);
        let turns = rng.range(1, TURNS_MAX);
        let store = options.store.unwrap_or(if rng.chance(crate::rng::PPM / 2) {
            StoreKind::Memory
        } else {
            StoreKind::Kv
        });
        let crashes = match store {
            StoreKind::Memory => Vec::new(),
            StoreKind::Kv => {
                let count = rng.range(0, CRASHES_MAX);
                let mut crashes: Vec<u64> = (0..count)
                    .map(|_| rng.range(0, CRASH_HORIZON_MS) * 1_000_000)
                    .collect();
                crashes.sort_unstable();
                crashes
            }
        };
        let stop = if rng.chance(STOP_PPM) {
            Stop::Cancel
        } else if rng.chance(STOP_PPM) {
            Stop::Close
        } else {
            Stop::Never
        };
        let stop_after = Duration::from_millis(rng.range(0, STOP_AFTER_MS_MAX));
        let hibernation = rng.chance(HIBERNATION_PPM).then(|| {
            usize::try_from(rng.range(0, RESIDENT_MAX)).expect("a resident bound fits in usize")
        });
        Self {
            stop,
            stop_after,
            hibernation,
            roots,
            turns,
            store,
            crashes,
            faults: NetworkFaults::of(seed, options.calm),
            http_faults: HttpFaults::of(seed, options.calm),
            wire: options.wire,
        }
    }

    fn describe(&self) -> String {
        format!(
            "scenario=runtime wire={} stop={:?} stop_after_ms={} resident<={:?} roots={} turns={} store={} crashes_ms={:?} calm_network={}",
            self.wire.name(),
            self.stop,
            self.stop_after.as_millis(),
            self.hibernation,
            self.roots,
            self.turns,
            self.store.name(),
            self.crashes.iter().map(|at| at / 1_000_000).collect::<Vec<_>>(),
            match self.wire {
                WireKind::Transport => self.faults.calm(),
                WireKind::Http => self.http_faults.calm(),
            }
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WholeStats {
    pub seeds: u64,
    pub crashes: u64,
    pub epochs: u64,
    pub turns: u64,
    pub failed_turns: u64,
    pub world: WorldCounts,
}

impl WholeStats {
    pub const fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.crashes += other.crashes;
        self.epochs += other.epochs;
        self.turns += other.turns;
        self.failed_turns += other.failed_turns;
        self.world.requests += other.world.requests;
        self.world.effects += other.world.effects;
        self.world.acked += other.world.acked;
        self.world.spawned += other.world.spawned;
        self.world.submitted += other.world.submitted;
        self.world.waits += other.world.waits;
        self.world.faults += other.world.faults;
        self.world.connections += other.world.connections;
        self.world.reuses += other.world.reuses;
    }
}

pub struct WholeOutcome {
    pub lines: Vec<String>,
    pub digest: u64,
    pub wrong: Option<String>,
    pub stats: WholeStats,
    pub shape: String,
}

struct EffectTool(Arc<World>);

impl nanocode::FunctionHandler for EffectTool {
    fn call<'a>(
        &'a self,
        _name: &'a str,
        input: Value,
        _context: nanocode::tools::ToolContext<'a>,
    ) -> nanocode::HostFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let key = input
                .get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| "sim_effect needs a key".to_owned())?;
            self.0.effect(key);
            Ok(json!({"applied": key}))
        })
    }
}

fn tools(world: &Arc<World>) -> Tools {
    Tools::builder()
        .without_defaults()
        .tool(nanocode::FunctionTool::new(
            EFFECT_TOOL,
            "Apply a simulated external effect once",
            json!({"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"]}),
            Arc::new(EffectTool(Arc::clone(world))),
        ))
        .build()
        .expect("the simulated tools build")
}

struct Durable {
    kind: StoreKind,
    kv: Option<SimKv>,
}

impl Durable {
    fn session_store(&self) -> Result<SessionStore, String> {
        match self.kind {
            StoreKind::Memory => MemoryStore::new()
                .map_err(|error| format!("MemoryStore::new failed: {error}"))
                .and_then(|store| {
                    nanocode::session_store(store).map_err(|error| format!("session store: {error}"))
                }),
            StoreKind::Kv => nanocode::session_store(KeyValueStore::new(
                self.kv.clone().expect("a kv seed has a kv"),
            ))
            .map_err(|error| format!("session store: {error}")),
        }
    }
}

fn claude(wire: &Wire) -> Result<ClaudeClient, String> {
    match wire {
        Wire::Transport(network) => Ok(ClaudeClient::messages_api(ENDPOINT, "sk-sim", CONNECT_TIMEOUT, IDLE_TIMEOUT)
            .map_err(|error| format!("claude client: {error}"))?
            .with_transport(Transport(Arc::clone(network)))),
        Wire::Http(server) => {
            let builder = nanocode_http::Client::builder()
                .no_proxy()
                .connect_timeout(CONNECT_TIMEOUT)
                .default_headers(nanocode_http::accept_encoding(nanocode_http::header::HeaderMap::new()))
                .connector(Connector(Arc::clone(server)));
            let builder = match server.mutation() {
                WireMutation::NoReadTimeout => builder,
                _ => builder.read_timeout(IDLE_TIMEOUT),
            };
            let http = builder.build().map_err(|error| format!("http client: {error}"))?;
            Ok(ClaudeClient::new(http, MESSAGES_URL, "sk-sim"))
        }
    }
}

fn runtime(store: SessionStore, wire: &Wire, world: &Arc<World>) -> Result<Nanocode, String> {
    let claude = claude(wire)?;
    Nanocode::builder()
        .claude(claude)
        .tools(tools(world))
        .runner_management(false)
        .thinking(Thinking::Low)
        .workspace(PathBuf::from("/nonexistent/nanocode-sim"))
        .durability(store)
        .build()
        .map_err(|error| format!("runtime build: {error}"))
}

fn session_of(root: u64) -> String {
    format!("sim-root-{root}")
}

fn label_of(root: u64) -> String {
    format!("r{root}")
}

struct Client {
    failed: RefCell<Vec<(String, u64)>>,
    stopped: std::cell::Cell<bool>,
}

async fn open_root(
    runtime: &Nanocode,
    root: u64,
    epoch: u64,
    world: &World,
) -> Result<nanocode::Agent, String> {
    let session = session_of(root);
    if epoch > 0 {
        match runtime.open_session(&session).await {
            Ok((agent, _events)) => {
                world.trace(&format!("opened {session}"));
                if let Some(control) = runtime.subagents() {
                    control.await_restored(&agent, &session).await;
                }
                return Ok(agent);
            }
            Err(error) => world.trace(&format!("open {session} failed: {error}")),
        }
    }
    let (agent, _events) = runtime
        .spawn()
        .backend::<nanocode::Claude>()
        .model(nanocode::ClaudeModel::Haiku55)
        .session_id(session.clone())
        .build()
        .await
        .map_err(|error| format!("spawn {session}: {error}"))?;
    world.trace(&format!("spawned {session}"));
    Ok(agent)
}

async fn submit(agent: &nanocode::Agent, label: &str, turn: u64) -> Result<String, String> {
    let request_id = format!("turn-{turn}");
    let submitted = agent
        .prompt(PromptRequest::new(root_prompt(label, turn)).request_id(&request_id))
        .await;
    match submitted {
        Ok(running) => running
            .result()
            .await
            .map(|result| result.final_message().to_owned())
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

async fn stored_status(store: &SessionStore, session: &str, request_id: &str) -> Option<TurnStatus> {
    store
        .turn(session, request_id)
        .await
        .ok()
        .flatten()
        .map(|turn| turn.status)
}

async fn run_turn(
    agent: &nanocode::Agent,
    store: &SessionStore,
    label: &str,
    turn: u64,
    world: &World,
) -> Result<String, String> {
    let session = agent.session_id().to_owned();
    let request_id = format!("turn-{turn}");
    let mut attempt = 0;
    loop {
        world.trace(&format!("submit {label} {request_id} attempt {attempt}"));
        let error = match submit(agent, label, turn).await {
            Ok(text) => return Ok(text),
            Err(error) => error,
        };
        let status = stored_status(store, &session, &request_id).await;
        world.trace(&format!(
            "{label} turn {turn} attempt {attempt} failed ({status:?}): {error}"
        ));
        if !matches!(status, Some(TurnStatus::Pending)) || attempt >= RESUBMITS_MAX {
            return Err(error);
        }
        attempt += 1;
        nanocode_platform::sleep(RESUBMIT_DELAY).await;
    }
}

async fn run_root(
    runtime: &Nanocode,
    store: &SessionStore,
    shape: &Shape,
    root: u64,
    epoch: u64,
    world: &World,
    client: &Client,
) -> Result<nanocode::Agent, String> {
    let agent = open_root(runtime, root, epoch, world).await?;
    let label = label_of(root);
    for turn in 0..shape.turns {
        if world.acked(&label).contains_key(&turn)
            || client.failed.borrow().contains(&(label.clone(), turn))
        {
            continue;
        }
        match run_turn(&agent, store, &label, turn, world).await {
            Ok(text) => {
                agent
                    .flush()
                    .await
                    .map_err(|error| format!("{label} flush after turn {turn}: {error}"))?;
                if text != final_text(&label, turn) {
                    world.wrong(format!(
                        "{label} turn {turn} answered {text:?}, not the model's final text"
                    ));
                }
                world.ack(&label, turn, &text);
            }
            Err(error) => {
                world.trace(&format!("{label} turn {turn} failed: {error}"));
                client.failed.borrow_mut().push((label.clone(), turn));
            }
        }
    }
    Ok(agent)
}

async fn settle(
    runtime: &Nanocode,
    store: &SessionStore,
    shape: &Shape,
    world: &World,
) -> Result<(), String> {
    let started = nanocode_platform::now();
    for root in 0..shape.roots {
        let session = session_of(root);
        loop {
            match runtime.settle(&session).await {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => return Err(format!("settle {session}: {error}")),
            }
            if nanocode_platform::now().duration_since(started) >= SETTLE_DEADLINE {
                let activity = match runtime.subagents() {
                    Some(control) => format!("{:?}", control.activity(&session).await),
                    None => "no subagents".to_owned(),
                };
                let pending = runtime.pending_turns(&session).await;
                let turns = store.load(&session).await.map(|stored| {
                    stored
                        .turns
                        .iter()
                        .map(|turn| format!("{}={:?}", turn.id, turn.status))
                        .collect::<Vec<_>>()
                });
                return Err(format!(
                    "{session} did not settle within {} s of virtual time: activity {activity}, pending turns {pending:?}, stored turns {turns:?}",
                    SETTLE_DEADLINE.as_secs()
                ));
            }
            nanocode_platform::sleep(SETTLE_POLL).await;
        }
        world.trace(&format!("settled {session}"));
    }
    Ok(())
}

async fn stop(
    runtime: &Nanocode,
    shape: &Shape,
    world: &World,
    client: &Client,
) -> Result<(), String> {
    if shape.stop == Stop::Never || client.stopped.get() {
        return Ok(());
    }
    nanocode_platform::sleep(shape.stop_after).await;
    if client.stopped.replace(true) {
        return Ok(());
    }
    let Some(control) = runtime.subagents() else {
        return Ok(());
    };
    for root in 0..shape.roots {
        let session = session_of(root);
        world.trace(&format!("{:?} {session}", shape.stop));
        match shape.stop {
            Stop::Cancel => control.cancel_all(&session).await,
            Stop::Close => control
                .close_all(&session)
                .await
                .map_err(|error| format!("close_all {session}: {error}"))?,
            Stop::Never => {}
        }
        world.trace(&format!("{:?} {session} returned", shape.stop));
    }
    Ok(())
}

async fn settled(runtime: &Nanocode, shape: &Shape, world: &World) -> Result<(), String> {
    let Some(control) = runtime.subagents() else {
        return Ok(());
    };
    for root in 0..shape.roots {
        let session = session_of(root);
        let listed = control
            .agents(&session)
            .await
            .map_err(|error| format!("the directory of {session} did not answer: {error}"))?;
        for entry in listed {
            world.trace(&format!(
                "{session} agent {} {:?}",
                entry.agent_id,
                std::mem::discriminant(&entry.status)
            ));
            if entry.status.is_active() {
                return Err(format!(
                    "{session} agent {} is {:?} after its root settled",
                    entry.agent_id, entry.status
                ));
            }
        }
    }
    Ok(())
}

async fn epoch(
    durable: &Durable,
    shape: &Shape,
    epoch: u64,
    wire: &Wire,
    world: &Arc<World>,
    client: &Client,
) -> Result<(), String> {
    world.trace(&format!("epoch {epoch}"));
    nanocode::claude::durability::forget_templates();
    let store = durable.session_store()?;
    let runtime = runtime(store.clone(), wire, world)?;
    if let (Some(control), Some(resident_max)) = (runtime.subagents(), shape.hibernation) {
        control.set_hibernation(true);
        control.set_max_resident(resident_max);
    }
    let roots = async {
        let mut agents = Vec::new();
        for root in 0..shape.roots {
            agents.push(run_root(&runtime, &store, shape, root, epoch, world, client).await?);
        }
        Ok::<_, String>(agents)
    };
    let (agents, stopped) =
        futures_util::future::join(roots, stop(&runtime, shape, world, client)).await;
    let agents = agents?;
    stopped?;
    settle(&runtime, &store, shape, world).await?;
    settled(&runtime, shape, world).await?;
    drop(agents);
    runtime
        .shutdown()
        .await
        .map_err(|error| format!("shutdown: {error}"))?;
    world.trace("shut down");
    Ok(())
}

fn drop_spawned() {
    loop {
        let spawned = crate::host::spawned();
        if spawned.is_empty() {
            return;
        }
        drop(spawned);
    }
}

pub fn simulate(seed: u64, options: WholeOptions) -> WholeOutcome {
    let shape = Shape::of(seed, options);
    if std::env::var_os("NANOCODE_SIM_POLLS").is_some() {
        eprintln!("RUN seed {seed}");
    }
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let world = World::new(Arc::clone(&clock), options.verbose);
    let wire = match shape.wire {
        WireKind::Transport => Wire::Transport(Arc::new(Network::new(seed, Arc::clone(&world), shape.faults, options.mutation))),
        WireKind::Http => Wire::Http(Arc::new(HttpServer::new(seed, Arc::clone(&world), shape.http_faults, options.mutation, IDLE_TIMEOUT))),
    };
    let durable = Durable {
        kind: shape.store,
        kv: (shape.store == StoreKind::Kv).then(|| {
            let mut profile = Profile::of(seed);
            profile.before_ppm = 0;
            profile.after_ppm = 0;
            profile.torn_ppm = 0;
            profile.read_ppm = 0;
            profile.durability = crate::faults::Durability::OnAck;
            SimKv::new(seed, profile, Mutation::None, false, Arc::clone(&clock))
        }),
    };
    let client = Client {
        failed: RefCell::new(Vec::new()),
        stopped: std::cell::Cell::new(false),
    };
    let mut crashes: VecDeque<u64> = shape.crashes.iter().copied().collect();
    let mut stats = WholeStats {
        seeds: 1,
        ..WholeStats::default()
    };
    let mut verdict = None;
    for number in 0..EPOCHS_MAX {
        stats.epochs += 1;
        let result = RefCell::new(None);
        let mut executor = Executor::new(
            Arc::clone(&clock),
            Rng::derive(seed, &format!("scheduler {number}")),
        );
        let task: Task<'_> = Box::pin(async {
            let outcome = epoch(&durable, &shape, number, &wire, &world, &client).await;
            result.replace(Some(outcome));
        });
        let crash_at = crashes.pop_front();
        let ended = executor.run(vec![task], crash_at);
        drop(executor);
        drop_spawned();
        match (ended, result.into_inner()) {
            (Ended::Crashed, _) => {
                stats.crashes += 1;
                world.trace(&format!("crash in epoch {number}"));
                if let Some(kv) = &durable.kv {
                    let lost = kv.with(crate::store::Kv::power_loss);
                    world.trace(&format!(
                        "power loss reverted {} writes, survived through #{}",
                        lost.reverted, lost.survived_seq
                    ));
                }
            }
            (_, Some(outcome)) => {
                verdict = Some(outcome);
                break;
            }
            (Ended::Stuck, None) => {
                verdict = Some(Err(format!(
                    "epoch {number}: every task waits and no timer is set"
                )));
                break;
            }
            (Ended::Finished, None) => {
                verdict = Some(Err(format!("epoch {number} finished without a verdict")));
                break;
            }
        }
    }
    let verdict = verdict.unwrap_or_else(|| Err("the run never ended".to_owned()));
    stats.world = world.counts();
    stats.failed_turns = u64::try_from(client.failed.borrow().len()).expect("a count fits in u64");
    stats.turns = shape.roots * shape.turns;
    let wrong = world.first_violation().or(verdict.err());
    WholeOutcome {
        lines: world.lines(),
        digest: world.digest(),
        wrong,
        stats,
        shape: shape.describe(),
    }
}

pub fn run_seed(seed: u64, options: WholeOptions) -> WholeOutcome {
    std::panic::catch_unwind(|| simulate(seed, options)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_default();
        WholeOutcome {
            lines: Vec::new(),
            digest: 0,
            wrong: Some(format!("panic: {message}")),
            stats: WholeStats::default(),
            shape: String::new(),
        }
    })
}

pub fn divergence(first: &[String], second: &[String]) -> String {
    let at = first
        .iter()
        .zip(second)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| first.len().min(second.len()));
    format!(
        "first divergence at trace line {at}: {:?} then {:?}",
        first.get(at),
        second.get(at)
    )
}
