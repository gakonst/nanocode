use std::{
    cell::RefCell,
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use nanocode::{
    ClaudeClient, Nanocode, PromptRequest, Thinking, Tools,
    durability::{MemoryStore, SessionStore, TurnStatus},
};
use nanocode_durability_core::KeyValueStore;
use serde_json::{Value, json};

use crate::{
    children::{ChildDisk, ChildFaults, ChildMutation, Host, SimChildren},
    disk::Disk,
    executor::{Clock, Ended, Executor, Task},
    faults::Profile,
    httpd::{Connector, HttpFaults, HttpServer, WireMutation},
    network::{Network, NetworkFaults, Transport},
    rng::Rng,
    server::{CHILD_MARK, EFFECT_TOOL, final_text, root_prompt, word_after},
    store::{Mutation, SimKv},
    turnlog::SimHost,
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
pub const CANCEL_TURN_PPM: u64 = 300_000;
pub const CANCEL_POLL: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const ENDPOINT: &str = "http://model.sim/v1";
const MESSAGES_URL: &str = "http://model.sim/v1/messages";
const EPOCHS_MAX: u64 = CRASHES_MAX + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    Memory,
    Kv,
    Log,
}

impl StoreKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "memory" => Some(Self::Memory),
            "kv" => Some(Self::Kv),
            "log" => Some(Self::Log),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Kv => "kv",
            Self::Log => "log",
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChildMode {
    #[default]
    Local,
    Remote,
}

impl ChildMode {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "local" => Some(Self::Local),
            "remote" => Some(Self::Remote),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
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
    pub children: ChildMode,
    pub child_mutation: ChildMutation,
}

impl Default for WholeOptions {
    fn default() -> Self {
        Self {
            store: None,
            calm: false,
            verbose: false,
            wire: WireKind::Transport,
            mutation: WireMutation::None,
            children: ChildMode::Local,
            child_mutation: ChildMutation::None,
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
    cancel_turn: Option<(u64, u64)>,
    children: ChildMode,
    child_faults: ChildFaults,
}

impl Shape {
    fn of(seed: u64, options: WholeOptions) -> Self {
        let mut rng = Rng::derive(seed, "whole-shape");
        let roots = rng.range(1, ROOTS_MAX);
        let turns = rng.range(1, TURNS_MAX);
        let store = options.store.unwrap_or_else(|| match options.children {
            ChildMode::Remote => StoreKind::Log,
            ChildMode::Local if rng.chance(crate::rng::PPM / 2) => StoreKind::Memory,
            ChildMode::Local => StoreKind::Kv,
        });
        let crashes = match store {
            StoreKind::Memory => Vec::new(),
            StoreKind::Kv | StoreKind::Log => {
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
        let mut cancels = Rng::derive(seed, "whole-cancel");
        let cancel_turn = cancels
            .chance(CANCEL_TURN_PPM)
            .then(|| (cancels.range(0, roots - 1), cancels.range(0, turns - 1)));
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
            cancel_turn,
            children: options.children,
            child_faults: ChildFaults::of(seed, options.calm),
        }
    }

    fn describe(&self) -> String {
        format!(
            "scenario=runtime wire={} children={} stop={:?} stop_after_ms={} resident<={:?} roots={} turns={} store={} crashes_ms={:?} calm_network={} cancel_turn={:?}{}",
            self.wire.name(),
            self.children.name(),
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
            },
            self.cancel_turn,
            match self.children {
                ChildMode::Local => String::new(),
                ChildMode::Remote => format!(
                    " child_crashes_ms={:?} kill_after_ms={} calm_children={}",
                    self.child_faults.crashes.iter().map(|at| at / 1_000_000).collect::<Vec<_>>(),
                    self.child_faults.kill_after.as_millis(),
                    self.child_faults.calm()
                ),
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
        self.world.child_crashes += other.world.child_crashes;
        self.world.child_model_calls += other.world.child_model_calls;
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

fn tools(world: &Arc<World>, parts: Option<&nanocode::turnlog::Parts>) -> Tools {
    let builder = Tools::builder()
        .without_defaults()
        .tool(nanocode::FunctionTool::new(
            EFFECT_TOOL,
            "Apply a simulated external effect once",
            json!({"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"]}),
            Arc::new(EffectTool(Arc::clone(world))),
        ));
    let builder = match parts {
        Some(parts) => builder
            .effect_gate(Arc::clone(&parts.gate))
            .cell_journals(parts.journals.clone()),
        None => builder,
    };
    builder.build().expect("the simulated tools build")
}

struct Durable {
    kind: StoreKind,
    kv: Option<SimKv>,
    disk: Arc<Disk>,
    settle_durably: bool,
    world: Arc<World>,
}

impl Durable {
    async fn session_store(&self) -> Result<(SessionStore, Option<nanocode::turnlog::Parts>), String> {
        let store = match self.kind {
            StoreKind::Memory => MemoryStore::new()
                .map_err(|error| format!("MemoryStore::new failed: {error}"))
                .and_then(|store| {
                    nanocode::session_store(store).map_err(|error| format!("session store: {error}"))
                }),
            StoreKind::Kv => nanocode::session_store(KeyValueStore::new(
                self.kv.clone().expect("a kv seed has a kv"),
            ))
            .map_err(|error| format!("session store: {error}")),
            StoreKind::Log => {
                let host: Arc<dyn nanocode::turnlog::LogHost> =
                    Arc::new(TracedHost(SimHost::new(Arc::clone(&self.disk)), Arc::clone(&self.world)));
                let policy = nanocode::turnlog::Policy {
                    effects: nanocode::turnlog::Effects::default(),
                    max_row_bytes: None,
                    restarts: u64::MAX,
                    retain_finished: None,
                    snapshot_ratio: 2,
                    snapshot_rows: None,
                };
                let parts = nanocode::turnlog(host, "sim", policy)
                    .await
                    .map_err(|error| format!("turn log: {error}"))?;
                if self.settle_durably {
                    parts.log.settle_durably();
                }
                return Ok((parts.store.clone(), Some(parts)));
            }
        };
        store.map(|store| (store, None))
    }
}

struct TracedHost(SimHost, Arc<World>);

impl nanocode::turnlog::LogHost for TracedHost {
    fn scan<'a>(
        &'a self,
        scope: &'a str,
        from: u64,
        to: u64,
    ) -> nanocode_durability_core::StoreFuture<'a, Result<Vec<(u64, String)>, nanocode_durability_core::StoreError>> {
        self.0.scan(scope, from, to)
    }

    fn load<'a>(
        &'a self,
        scope: &'a str,
    ) -> nanocode_durability_core::StoreFuture<'a, Result<Vec<(u64, String)>, nanocode_durability_core::StoreError>> {
        self.0.load(scope)
    }

    fn append<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
    ) -> nanocode_durability_core::StoreFuture<'a, Result<(), nanocode_durability_core::StoreError>> {
        self.1.trace(&format!(
            "log append {} rows {}",
            rows.len(),
            rows.iter().map(|(_, body)| body.chars().take(60).collect::<String>()).collect::<Vec<_>>().join(" | ")
        ));
        self.0.append(scope, rows)
    }
}

struct Remote {
    seed: u64,
    clock: Arc<Clock>,
    disk: Arc<ChildDisk>,
    faults: ChildFaults,
    mutation: ChildMutation,
}

impl Remote {
    fn children(&self, epoch: u64, world: &Arc<World>) -> Arc<SimChildren> {
        SimChildren::new(
            self.seed,
            epoch,
            Arc::clone(&self.clock),
            Arc::clone(&self.disk),
            Arc::clone(world),
            self.faults.clone(),
            self.mutation,
        )
    }
}

type Listing = Vec<(String, Vec<String>)>;

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

fn runtime(
    store: SessionStore,
    parts: Option<&nanocode::turnlog::Parts>,
    wire: &Wire,
    world: &Arc<World>,
    children: Option<&Arc<SimChildren>>,
) -> Result<Nanocode, String> {
    let claude = claude(wire)?;
    let builder = Nanocode::builder()
        .claude(claude)
        .tools(tools(world, parts))
        .runner_management(false)
        .thinking(Thinking::Low)
        .workspace(PathBuf::from("/nonexistent/nanocode-sim"))
        .durability(store);
    let builder = match children {
        Some(children) => builder.child_host(Host(Arc::clone(children))),
        None => builder,
    };
    let runtime = builder
        .build()
        .map_err(|error| format!("runtime build: {error}"))?;
    if let Some(children) = children {
        children.attach(
            runtime
                .remote_children()
                .ok_or_else(|| "a runtime with a child host has no remote children".to_owned())?,
        );
        children.start();
    }
    Ok(runtime)
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

async fn submit(
    agent: &nanocode::Agent,
    label: &str,
    turn: u64,
    cancel: bool,
    world: &World,
) -> Result<String, String> {
    let request_id = format!("turn-{turn}");
    let submitted = agent
        .prompt(PromptRequest::new(root_prompt(label, turn)).request_id(&request_id))
        .await;
    let running = submitted.map_err(|error| error.to_string())?;
    if !cancel {
        return running
            .result()
            .await
            .map(|result| result.final_message().to_owned())
            .map_err(|error| error.to_string());
    }
    let control = running.control();
    let done = std::cell::Cell::new(false);
    let cancelled = std::cell::Cell::new(false);
    let result = async {
        let result = running.result().await;
        done.set(true);
        result
    };
    let canceller = async {
        while !done.get() {
            if world.saw(label, turn) {
                world.trace(&format!("cancel {label} turn {turn}"));
                cancelled.set(control.cancel().await.is_ok());
                return;
            }
            nanocode_platform::sleep(CANCEL_POLL).await;
        }
    };
    let (result, ()) = nanocode_sync::future::join(result, canceller).await;
    match result {
        Ok(result) => Ok(result.final_message().to_owned()),
        Err(nanocode::NanocodeError::TurnCancelled) if cancelled.get() => {
            world.cancelled(label, turn);
            Err(nanocode::NanocodeError::TurnCancelled.to_string())
        }
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
    cancel: bool,
    world: &World,
) -> Result<String, String> {
    let session = agent.session_id().to_owned();
    let request_id = format!("turn-{turn}");
    let mut attempt = 0;
    loop {
        world.trace(&format!("submit {label} {request_id} attempt {attempt}"));
        let error = match submit(agent, label, turn, cancel && attempt == 0, world).await {
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
        let cancel = shape.cancel_turn == Some((root, turn)) && !client.stopped.get();
        match run_turn(&agent, store, &label, turn, cancel, world).await {
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

async fn listing(runtime: &Nanocode, shape: &Shape) -> Result<(Listing, BTreeSet<String>), String> {
    let Some(control) = runtime.subagents() else {
        return Ok((Vec::new(), BTreeSet::new()));
    };
    let mut listing = Vec::new();
    let mut completed = BTreeSet::new();
    for root in 0..shape.roots {
        let session = session_of(root);
        let entries = control
            .agents(&session)
            .await
            .map_err(|error| format!("the directory of {session} did not answer: {error}"))?;
        let mut lines = Vec::new();
        for entry in &entries {
            if matches!(entry.status, nanocode::AgentStatus::Completed { .. })
                && let Some(label) = word_after(&entry.task, CHILD_MARK)
            {
                completed.insert(label.to_owned());
            }
            lines.push(
                serde_json::to_string(entry)
                    .map_err(|error| format!("agent {} does not encode: {error}", entry.agent_id))?,
            );
        }
        listing.push((session, lines));
    }
    Ok((listing, completed))
}

fn difference(before: &Listing, after: &Listing) -> Option<String> {
    for ((session, lines), (_, restored)) in before.iter().zip(after) {
        if let Some(at) = (0..lines.len().max(restored.len())).find(|at| lines.get(*at) != restored.get(*at)) {
            return Some(format!(
                "{session} entry {at}: {:?} before the restart, {:?} after",
                lines.get(at),
                restored.get(at)
            ));
        }
    }
    (before.len() != after.len()).then(|| format!("{} roots before the restart, {} after", before.len(), after.len()))
}

async fn verify(
    durable: &Durable,
    shape: &Shape,
    number: u64,
    wire: &Wire,
    world: &Arc<World>,
    remote: Option<&Remote>,
    before: Listing,
) -> Result<(), String> {
    world.trace(&format!("epoch {number} verifies the restart"));
    nanocode::claude::durability::forget_templates();
    let (store, parts) = durable.session_store().await?;
    let children = remote.map(|remote| remote.children(number, world));
    let runtime = runtime(store.clone(), parts.as_ref(), wire, world, children.as_ref())?;
    let mut agents = Vec::new();
    for root in 0..shape.roots {
        agents.push(open_root(&runtime, root, number, world).await?);
    }
    let (after, completed) = listing(&runtime, shape).await?;
    if shape.stop != Stop::Close
        && let Some(difference) = difference(&before, &after)
    {
        world.wrong(format!("remote_list_changed: {difference}"));
    }
    settle(&runtime, &store, shape, world).await?;
    if let Some(children) = &children {
        let runs = children.run_calls();
        if shape.stop != Stop::Close && runs > 0 {
            world.wrong(format!(
                "remote_restart_reran: the parent ran {runs} child turns after a restart of settled roots"
            ));
        }
        if shape.stop == Stop::Never
            && shape.cancel_turn.is_none()
            && let Some(lost) = children.lost(&completed)
        {
            world.wrong(format!("remote_turn_lost: {lost}"));
        }
    }
    drop(agents);
    runtime
        .shutdown()
        .await
        .map_err(|error| format!("shutdown: {error}"))?;
    world.trace("shut down");
    Ok(())
}

async fn epoch(
    durable: &Durable,
    shape: &Shape,
    epoch: u64,
    wire: &Wire,
    world: &Arc<World>,
    client: &Client,
    remote: Option<&Remote>,
) -> Result<Option<Listing>, String> {
    world.trace(&format!("epoch {epoch}"));
    nanocode::claude::durability::forget_templates();
    let (store, parts) = durable.session_store().await?;
    let children = remote.map(|remote| remote.children(epoch, world));
    let runtime = runtime(store.clone(), parts.as_ref(), wire, world, children.as_ref())?;
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
        nanocode_sync::future::join(roots, stop(&runtime, shape, world, client)).await;
    let agents = agents?;
    stopped?;
    settle(&runtime, &store, shape, world).await?;
    settled(&runtime, shape, world).await?;
    if children.is_some() && durable.kind != StoreKind::Memory {
        let (before, _) = listing(&runtime, shape).await?;
        nanocode_platform::sleep(shape.child_faults.kill_after).await;
        world.trace("killed after the roots settled");
        drop(agents);
        return Ok(Some(before));
    }
    drop(agents);
    runtime
        .shutdown()
        .await
        .map_err(|error| format!("shutdown: {error}"))?;
    world.trace("shut down");
    Ok(None)
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
    let remote = (shape.children == ChildMode::Remote).then(|| Remote {
        seed,
        clock: Arc::clone(&clock),
        disk: ChildDisk::new(),
        faults: shape.child_faults.clone(),
        mutation: options.child_mutation,
    });
    let durable = Durable {
        world: Arc::clone(&world),
        disk: Disk::new(),
        settle_durably: remote.is_some() && options.child_mutation != ChildMutation::SettlesLazily,
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
            let outcome = epoch(&durable, &shape, number, &wire, &world, &client, remote.as_ref()).await;
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
    let verdict = match verdict {
        Some(Ok(Some(before))) => {
            stats.epochs += 1;
            if let Some(kv) = &durable.kv {
                kv.with(crate::store::Kv::power_loss);
            }
            let result = RefCell::new(None);
            let mut executor = Executor::new(Arc::clone(&clock), Rng::derive(seed, "scheduler verify"));
            let task: Task<'_> = Box::pin(async {
                let outcome = verify(&durable, &shape, EPOCHS_MAX, &wire, &world, remote.as_ref(), before).await;
                result.replace(Some(outcome));
            });
            let ended = executor.run(vec![task], None);
            drop(executor);
            drop_spawned();
            result
                .into_inner()
                .unwrap_or_else(|| Err(format!("the verification epoch ended {ended:?} without a verdict")))
        }
        Some(outcome) => outcome.map(drop),
        None => Err("the run never ended".to_owned()),
    };
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
