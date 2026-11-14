use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Weak},
    time::Duration,
};

use nanocode::{
    ChildHost, ChildSpec, ChildToolCall, NanocodeError, RemoteChildren,
    backend::{BackendFuture, Prompt, PromptInput, ToolInput, TurnResult},
};
use nanocode_sync::{
    Slot,
    future::{AbortHandle, Abortable},
    oneshot,
};
use serde_json::{Value, json};

use crate::{
    executor::Clock,
    rng::Rng,
    server::{
        CHILD_MARK, CHILDREN_MAX, WAIT_AGENT_TIMEOUT_MS, child_task, number_after, word_after,
    },
    world::World,
};

pub const CALM_PPM: u64 = 300_000;
pub const UNSENT_PPM_MAX: u64 = 60_000;
pub const LOST_PPM_MAX: u64 = 60_000;
pub const DUPLICATE_PPM_MAX: u64 = 60_000;
pub const LATENCY_MS_MAX: u64 = 2_000;
pub const STEP_MS_MAX: u64 = 3_000;
pub const CRASHES_MAX: u64 = 4;
pub const CRASH_HORIZON_MS: u64 = 120_000;
pub const RECOVERY_DELAY: Duration = Duration::from_secs(5);
pub const KILL_AFTER_MS_MAX: u64 = 10_000;
const SPAWN_PPM: u64 = 500_000;
const MESSAGE_PPM: u64 = 500_000;
const MESSAGE_PEERS: usize = 2;
const OUTPUT_BUDGET: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildMutation {
    None,
    AdoptsTurn,
    ForgetsNotice,
    SettlesLazily,
}

impl ChildMutation {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "child-adopts-turn" => Some(Self::AdoptsTurn),
            "child-forgets-notice" => Some(Self::ForgetsNotice),
            "child-settles-lazily" => Some(Self::SettlesLazily),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::AdoptsTurn => "child-adopts-turn",
            Self::ForgetsNotice => "child-forgets-notice",
            Self::SettlesLazily => "child-settles-lazily",
        }
    }

    pub const NAMES: &'static str =
        "child-adopts-turn|child-forgets-notice|child-settles-lazily";
}

#[derive(Clone, Debug)]
pub struct ChildFaults {
    pub unsent_ppm: u64,
    pub lost_ppm: u64,
    pub duplicate_ppm: u64,
    pub latency_ms_max: u64,
    pub crashes: Vec<u64>,
    pub kill_after: Duration,
}

impl ChildFaults {
    pub fn of(seed: u64, calm: bool) -> Self {
        let mut rng = Rng::derive(seed, "child-faults");
        let latency_ms_max = rng.range(0, LATENCY_MS_MAX);
        let kill_after = Duration::from_millis(rng.range(0, KILL_AFTER_MS_MAX));
        let count = rng.range(0, CRASHES_MAX);
        let mut crashes: Vec<u64> = (0..count)
            .map(|_| rng.range(0, CRASH_HORIZON_MS) * 1_000_000)
            .collect();
        crashes.sort_unstable();
        if calm || rng.chance(CALM_PPM) {
            return Self {
                unsent_ppm: 0,
                lost_ppm: 0,
                duplicate_ppm: 0,
                latency_ms_max,
                crashes,
                kill_after,
            };
        }
        Self {
            unsent_ppm: rng.range(0, UNSENT_PPM_MAX),
            lost_ppm: rng.range(0, LOST_PPM_MAX),
            duplicate_ppm: rng.range(0, DUPLICATE_PPM_MAX),
            latency_ms_max,
            crashes,
            kill_after,
        }
    }

    pub const fn calm(&self) -> bool {
        self.unsent_ppm == 0 && self.lost_ppm == 0 && self.duplicate_ppm == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fate {
    Delivered,
    Unsent,
    Lost,
    Duplicated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Spawn(Vec<String>),
    Message,
    Submit,
}

#[derive(Clone, Debug)]
enum Outcome {
    Completed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
struct Notice {
    request: String,
    outcome: Outcome,
    delivered: bool,
}

#[derive(Clone, Debug)]
struct Input {
    request: String,
    prompt: String,
    revision: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct Child {
    label: String,
    plan: Vec<Step>,
    cursor: usize,
    in_flight: Option<usize>,
    inputs: Vec<Input>,
    notices: Vec<Notice>,
    running: Option<String>,
    queued: VecDeque<String>,
    aliases: BTreeMap<String, String>,
    received: Vec<String>,
    steers: BTreeSet<String>,
}

impl Child {
    fn input(&self, request: &str) -> Option<&Input> {
        self.inputs.iter().find(|input| input.request == request)
    }

    fn notice(&self, request: &str) -> Option<usize> {
        self.notices
            .iter()
            .position(|notice| notice.request == request)
    }

    fn start(&mut self, request: &str) {
        self.running = Some(request.to_owned());
        if let Some(input) = self.input(request) {
            let prompt = input.prompt.clone();
            self.received.push(prompt);
        }
    }

    fn finish(&mut self, outcome: Outcome) -> Option<String> {
        let own = self.running.take()?;
        let request = self.aliases.remove(&own).unwrap_or(own);
        self.notices.push(Notice {
            request: request.clone(),
            outcome,
            delivered: false,
        });
        if let Some(next) = self.queued.pop_front() {
            self.start(&next);
        }
        Some(request)
    }
}

#[derive(Default)]
struct Disk {
    children: BTreeMap<Arc<str>, Child>,
    accepted: Vec<(String, String)>,
}

pub struct ChildDisk(Slot<Disk>);

impl ChildDisk {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Slot::new(Some(Disk::default()))))
    }

    fn with<R>(&self, change: impl FnOnce(&mut Disk) -> R) -> R {
        let mut disk = self.0.take().expect(
            "the simulator polls one task at a time, so the child disk is never taken twice",
        );
        let result = change(&mut disk);
        assert!(self.0.put(disk).is_none(), "the child disk has one home");
        result
    }

    pub fn lost(&self, completed: &BTreeSet<String>) -> Option<String> {
        self.with(|disk| {
            disk.accepted.iter().find_map(|(target, text)| {
                if !completed.contains(target) {
                    return None;
                }
                let child = disk
                    .children
                    .values()
                    .find(|child| &child.label == target)?;
                (!child
                    .received
                    .iter()
                    .any(|prompt| prompt.contains(text.as_str())))
                .then(|| format!("{target} completed without the accepted message {text:?}"))
            })
        })
    }
}

#[derive(Default)]
struct Live {
    open: BTreeSet<Arc<str>>,
    waiters: BTreeMap<(Arc<str>, String), oneshot::Sender<Outcome>>,
    workers: BTreeMap<Arc<str>, (u64, AbortHandle)>,
    notifiers: BTreeMap<Arc<str>, Vec<AbortHandle>>,
    completed: BTreeSet<(Arc<str>, String)>,
    remote: Weak<RemoteChildren>,
    rng: Option<Rng>,
    next_worker: u64,
    run_calls: u64,
}

pub struct SimChildren {
    seed: u64,
    clock: Arc<Clock>,
    disk: Arc<ChildDisk>,
    live: Slot<Live>,
    world: Arc<World>,
    faults: ChildFaults,
    mutation: ChildMutation,
}

enum Reply {
    Finished(Outcome),
    Pending,
}

type Next = (String, Option<u64>, Option<(usize, Step, bool)>, String);

fn failed(detail: impl Into<String>) -> NanocodeError {
    NanocodeError::Backend {
        backend: "remote_child",
        source: Arc::new(std::io::Error::other(detail.into())),
    }
}

fn prompt_text(prompt: &Prompt) -> Result<String, NanocodeError> {
    match &prompt.instruction {
        PromptInput::Text(text) => Ok(text.clone()),
        PromptInput::Content(_) => Err(NanocodeError::UnsupportedCapability {
            capability: "remote_child_content_prompt",
        }),
    }
}

fn plan(seed: u64, label: &str, depth: u64) -> Vec<Step> {
    let mut rng = Rng::derive(seed, &format!("child plan {label}"));
    let mut steps = Vec::new();
    if depth > 0 && rng.chance(SPAWN_PPM) {
        let count = rng.range(1, CHILDREN_MAX);
        steps.push(Step::Spawn(
            (0..count)
                .map(|index| child_task(&format!("{label}.c{index}"), depth - 1))
                .collect(),
        ));
    }
    if rng.chance(MESSAGE_PPM) {
        steps.push(Step::Message);
    }
    steps.push(Step::Submit);
    steps
}

fn short(text: &str) -> String {
    text.chars()
        .take(2000)
        .collect::<String>()
        .replace('\n', " ")
}

impl SimChildren {
    pub fn new(
        seed: u64,
        epoch: u64,
        clock: Arc<Clock>,
        disk: Arc<ChildDisk>,
        world: Arc<World>,
        faults: ChildFaults,
        mutation: ChildMutation,
    ) -> Arc<Self> {
        Arc::new(Self {
            seed,
            clock,
            disk,
            live: Slot::new(Some(Live {
                rng: Some(Rng::derive(seed, &format!("children {epoch}"))),
                ..Live::default()
            })),
            world,
            faults,
            mutation,
        })
    }

    fn live<R>(&self, change: impl FnOnce(&mut Live) -> R) -> R {
        let mut live = self.live.take().expect(
            "the simulator polls one task at a time, so the live children are never taken twice",
        );
        let result = change(&mut live);
        assert!(
            self.live.put(live).is_none(),
            "the live children have one home"
        );
        result
    }

    fn draw<R>(&self, use_rng: impl FnOnce(&mut Rng) -> R) -> R {
        self.live(|live| {
            use_rng(
                live.rng
                    .as_mut()
                    .expect("the live children have a scheduler"),
            )
        })
    }

    pub fn attach(&self, remote: &Arc<RemoteChildren>) {
        let remote = Arc::downgrade(remote);
        self.live(|live| live.remote = remote);
    }

    pub fn run_calls(&self) -> u64 {
        self.live(|live| live.run_calls)
    }

    pub fn lost(&self, completed: &BTreeSet<String>) -> Option<String> {
        self.disk.lost(completed)
    }

    async fn link(&self) -> Fate {
        let latency = self.draw(|rng| rng.range(0, self.faults.latency_ms_max));
        nanocode_platform::sleep(Duration::from_millis(latency)).await;
        self.draw(|rng| {
            if rng.chance(self.faults.unsent_ppm) {
                Fate::Unsent
            } else if rng.chance(self.faults.lost_ppm) {
                Fate::Lost
            } else if rng.chance(self.faults.duplicate_ppm) {
                Fate::Duplicated
            } else {
                Fate::Delivered
            }
        })
    }

    pub fn start(self: &Arc<Self>) {
        let sessions: Vec<Arc<str>> = self
            .disk
            .with(|disk| disk.children.keys().cloned().collect());
        for session in sessions {
            let children = Arc::clone(self);
            nanocode_platform::spawn(Box::pin(async move {
                nanocode_platform::sleep(RECOVERY_DELAY).await;
                children.pump(&session);
            }));
        }
        let children = Arc::clone(self);
        let crashes = self.faults.crashes.clone();
        nanocode_platform::spawn(Box::pin(async move {
            for at in crashes {
                let now = children.clock.now_ns();
                if at <= now {
                    continue;
                }
                nanocode_platform::sleep(Duration::from_nanos(at - now)).await;
                children.crash_one();
            }
        }));
    }

    fn crash_one(self: &Arc<Self>) {
        let alive: Vec<Arc<str>> = self.live(|live| {
            live.workers
                .keys()
                .chain(live.notifiers.keys())
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        });
        if alive.is_empty() {
            self.world.trace("child crash finds no live child");
            return;
        }
        let session = Arc::clone(&alive[self.draw(|rng| rng.pick(alive.len()))]);
        self.live(|live| {
            if let Some((_, worker)) = live.workers.remove(&session) {
                worker.abort();
            }
            for notifier in live.notifiers.remove(&session).unwrap_or_default() {
                notifier.abort();
            }
        });
        self.world.trace(&format!("child {session} crashed"));
        self.world.count(|counts| counts.child_crashes += 1);
        let children = Arc::clone(self);
        nanocode_platform::spawn(Box::pin(async move {
            nanocode_platform::sleep(RECOVERY_DELAY).await;
            children.pump(&session);
        }));
    }

    fn pump(self: &Arc<Self>, session: &Arc<str>) {
        let (running, undelivered) = self.disk.with(|disk| {
            disk.children.get(session).map_or((false, false), |child| {
                (
                    child.running.is_some(),
                    child.notices.iter().any(|notice| !notice.delivered),
                )
            })
        });
        if running {
            self.work(session);
        }
        if undelivered {
            self.notify(session);
        }
    }

    fn work(self: &Arc<Self>, session: &Arc<str>) {
        if self.live(|live| live.workers.contains_key(session)) {
            return;
        }
        let (handle, registration) = AbortHandle::new_pair();
        let id = self.live(|live| {
            live.next_worker += 1;
            live.workers
                .insert(Arc::clone(session), (live.next_worker, handle));
            live.next_worker
        });
        let children = Arc::clone(self);
        let owned = Arc::clone(session);
        nanocode_platform::spawn(Box::pin(async move {
            let worked = Abortable::new(
                Arc::clone(&children).turns(Arc::clone(&owned)),
                registration,
            )
            .await;
            if worked.is_ok() {
                children.live(|live| {
                    if live
                        .workers
                        .get(&owned)
                        .is_some_and(|(current, _)| *current == id)
                    {
                        live.workers.remove(&owned);
                    }
                });
            }
        }));
    }

    fn notify(self: &Arc<Self>, session: &Arc<str>) {
        let (handle, registration) = AbortHandle::new_pair();
        self.live(|live| {
            live.notifiers
                .entry(Arc::clone(session))
                .or_default()
                .push(handle);
        });
        let children = Arc::clone(self);
        let owned = Arc::clone(session);
        nanocode_platform::spawn(Box::pin(async move {
            Abortable::new(children.notices(owned), registration)
                .await
                .ok();
        }));
    }

    async fn notices(self: Arc<Self>, session: Arc<str>) {
        loop {
            let undelivered: Vec<(String, Outcome)> = self.disk.with(|disk| {
                disk.children
                    .get(&session)
                    .map(|child| {
                        child
                            .notices
                            .iter()
                            .filter(|notice| !notice.delivered)
                            .map(|notice| (notice.request.clone(), notice.outcome.clone()))
                            .collect()
                    })
                    .unwrap_or_default()
            });
            if undelivered.is_empty() {
                return;
            }
            let mut retry = false;
            for (request, outcome) in undelivered {
                let fate = self.link().await;
                if fate == Fate::Unsent {
                    retry = true;
                    continue;
                }
                let delivered = self.deliver(&session, &request, outcome.clone());
                if fate == Fate::Duplicated {
                    self.deliver(&session, &request, outcome);
                }
                if fate == Fate::Lost {
                    retry = true;
                    continue;
                }
                if delivered {
                    self.disk.with(|disk| {
                        if let Some(notice) = disk.children.get_mut(&session).and_then(|child| {
                            child
                                .notices
                                .iter_mut()
                                .find(|notice| notice.request == request)
                        }) {
                            notice.delivered = true;
                        }
                    });
                }
            }
            if !retry {
                return;
            }
            nanocode_platform::sleep(RECOVERY_DELAY).await;
        }
    }

    fn deliver(&self, session: &Arc<str>, request: &str, outcome: Outcome) -> bool {
        let waiter = self.live(|live| {
            live.waiters
                .remove(&(Arc::clone(session), request.to_owned()))
        });
        self.world.trace(&format!(
            "child {session} done {request} delivered={}",
            waiter.is_some()
        ));
        waiter.is_some_and(|waiter| {
            drop(waiter.send(outcome));
            true
        })
    }

    fn next(&self, session: &Arc<str>) -> Option<Next> {
        self.disk.with(|disk| {
            let child = disk.children.get(session)?;
            let request = child.running.clone()?;
            let revision = child.input(&request).and_then(|input| input.revision);
            let step = child.plan.get(child.cursor).map(|step| {
                (
                    child.cursor,
                    step.clone(),
                    child.in_flight == Some(child.cursor),
                )
            });
            Some((request, revision, step, child.label.clone()))
        })
    }

    async fn turns(self: Arc<Self>, session: Arc<str>) {
        loop {
            let Some((request, revision, step, label)) = self.next(&session) else {
                return;
            };
            self.world.count(|counts| counts.child_model_calls += 1);
            self.world.trace(&format!(
                "child model {label} {request} step {:?}",
                step.as_ref().map(|(index, ..)| *index)
            ));
            let think = self.draw(|rng| rng.range(0, STEP_MS_MAX));
            nanocode_platform::sleep(Duration::from_millis(think)).await;
            let Some((index, step, recovering)) = step else {
                let finished = self.disk.with(|disk| {
                    disk.children.get_mut(&session).and_then(|child| {
                        child.finish(Outcome::Completed(format!("finished {label}")))
                    })
                });
                self.world
                    .trace(&format!("child {label} finished {finished:?}"));
                if self.mutation == ChildMutation::ForgetsNotice {
                    self.disk.with(|disk| {
                        if let Some(child) = disk.children.get_mut(&session) {
                            child.notices.pop();
                        }
                    });
                }
                self.notify(&session);
                continue;
            };
            if recovering {
                self.world.trace(&format!(
                    "child {label} step {index} outcome unknown after a crash"
                ));
            } else {
                self.disk.with(|disk| {
                    if let Some(child) = disk.children.get_mut(&session) {
                        child.in_flight = Some(index);
                    }
                });
                self.perform(&session, &label, &request, revision, index, step)
                    .await;
            }
            self.disk.with(|disk| {
                if let Some(child) = disk.children.get_mut(&session) {
                    child.cursor = index + 1;
                    child.in_flight = None;
                }
            });
        }
    }

    async fn tool(
        &self,
        session: &Arc<str>,
        request: &str,
        revision: Option<u64>,
        call_id: String,
        name: &'static str,
        input: Value,
    ) -> Result<Value, String> {
        let fate = self.link().await;
        if fate == Fate::Unsent {
            return Err(format!("{name} never reached the parent"));
        }
        let Some(remote) = self.live(|live| live.remote.upgrade()) else {
            return Err("the parent runtime is gone".to_owned());
        };
        let (sender, receiver) = oneshot::channel();
        let owned = Arc::clone(session);
        let turn = request.to_owned();
        let raw = serde_json::value::to_raw_value(&input).map_err(|error| error.to_string())?;
        nanocode_platform::spawn(Box::pin(async move {
            let output = remote
                .call_tool(
                    &owned,
                    ChildToolCall {
                        call_id: &call_id,
                        host_context: None,
                        input: ToolInput::Function(raw),
                        instruction_revision: revision,
                        name,
                        output_budget: OUTPUT_BUDGET,
                        turn_id: Some(&turn),
                    },
                )
                .await;
            drop(sender.send(output));
        }));
        let output = receiver
            .await
            .map_err(|_| format!("{name} was dropped by the parent"))?
            .map_err(|error| error.to_string())?;
        if fate == Fate::Lost {
            return Err(format!("{name}'s answer was lost"));
        }
        let text = match output.output.structured_result() {
            Value::String(text) => text,
            other => other.to_string(),
        };
        if !output.success {
            return Err(text);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    async fn perform(
        &self,
        session: &Arc<str>,
        label: &str,
        request: &str,
        revision: Option<u64>,
        index: usize,
        step: Step,
    ) {
        let call = |name: &str| format!("{label}.s{index}.{name}");
        match step {
            Step::Spawn(tasks) => {
                for (number, task) in tasks.iter().enumerate() {
                    let input = json!({"role": "worker", "task": task, "harness": null, "model": null, "thinking": null, "reasoning_mode": null, "output_contract": {"kind": "string"}});
                    let spawned = self
                        .tool(
                            session,
                            request,
                            revision,
                            call(&format!("spawn{number}")),
                            "spawn_agent",
                            input,
                        )
                        .await;
                    self.world.trace(&format!(
                        "child {label} spawn_agent -> {}",
                        short(&format!("{spawned:?}"))
                    ));
                    let Some(id) = spawned
                        .ok()
                        .and_then(|output| output.get("agent_id").and_then(Value::as_u64))
                    else {
                        continue;
                    };
                    let input = json!({"agent_ids": [id], "timeout_ms": WAIT_AGENT_TIMEOUT_MS});
                    let waited = self
                        .tool(
                            session,
                            request,
                            revision,
                            call(&format!("wait{number}")),
                            "wait_agent",
                            input,
                        )
                        .await;
                    self.world.trace(&format!(
                        "child {label} wait_agent -> {}",
                        short(&format!("{waited:?}"))
                    ));
                }
            }
            Step::Message => {
                let listed = self
                    .tool(
                        session,
                        request,
                        revision,
                        call("list"),
                        "list_agents",
                        json!({}),
                    )
                    .await;
                let peers: Vec<(u64, String)> = listed
                    .ok()
                    .and_then(|listed| listed.get("agents").and_then(Value::as_array).cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|agent| {
                        let id = agent.get("agent_id").and_then(Value::as_u64)?;
                        let task = agent
                            .get("task")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        Some((
                            id,
                            word_after(task, CHILD_MARK).unwrap_or_default().to_owned(),
                        ))
                    })
                    .take(MESSAGE_PEERS)
                    .collect();
                for (id, target) in peers {
                    let text = format!("hello from {label} to agent {id}");
                    let input = json!({"agent_id": id, "message": text});
                    let sent = self
                        .tool(
                            session,
                            request,
                            revision,
                            call(&format!("send{id}")),
                            "send_agent_message",
                            input,
                        )
                        .await;
                    self.world.trace(&format!(
                        "child {label} send_agent_message {id} -> {}",
                        short(&format!("{sent:?}"))
                    ));
                    if sent.is_ok_and(|receipt| receipt.get("disposition").is_some())
                        && !target.is_empty()
                    {
                        self.disk.with(|disk| disk.accepted.push((target, text)));
                    }
                }
            }
            Step::Submit => {
                let input = json!({"output": format!("{label} done")});
                let submitted = self
                    .tool(
                        session,
                        request,
                        revision,
                        call("submit"),
                        "submit_result",
                        input,
                    )
                    .await;
                self.world.trace(&format!(
                    "child {label} submit_result -> {}",
                    short(&format!("{submitted:?}"))
                ));
            }
        }
    }

    fn turn(
        self: &Arc<Self>,
        session: &Arc<str>,
        request: &str,
        prompt: &str,
        revision: Option<u64>,
    ) -> Reply {
        let seed = self.seed;
        let adopts = self.mutation == ChildMutation::AdoptsTurn;
        let (reply, start) = self.disk.with(|disk| {
            let child = disk.children.entry(Arc::clone(session)).or_insert_with(|| {
                let label = word_after(prompt, CHILD_MARK)
                    .unwrap_or_default()
                    .to_owned();
                let depth = number_after(prompt, " depth ").unwrap_or(0);
                Child {
                    plan: plan(seed, &label, depth),
                    label,
                    ..Child::default()
                }
            });
            let known = child.input(request).is_some();
            let notice = child.notice(request);
            if !known && notice.is_none() {
                let delivered: Vec<String> = child
                    .notices
                    .iter()
                    .filter(|notice| notice.delivered)
                    .map(|notice| notice.request.clone())
                    .collect();
                child.notices.retain(|notice| !notice.delivered);
                child
                    .inputs
                    .retain(|input| !delivered.contains(&input.request));
            }
            let adopted = (adopts && !known)
                .then(|| child.notices.iter().rposition(|notice| !notice.delivered))
                .flatten();
            if let Some(at) = notice.or(adopted) {
                child.notices[at].delivered = true;
                return (Reply::Finished(child.notices[at].outcome.clone()), false);
            }
            if adopts
                && !known
                && let Some(running) = child.running.clone()
            {
                child.aliases.insert(running, request.to_owned());
                return (Reply::Pending, true);
            }
            if !known {
                child.inputs.push(Input {
                    request: request.to_owned(),
                    prompt: prompt.to_owned(),
                    revision,
                });
            }
            match child.running.as_deref() {
                Some(running) if running == request => {}
                Some(_) => {
                    if !child.queued.iter().any(|queued| queued == request) {
                        child.queued.push_back(request.to_owned());
                    }
                }
                None => child.start(request),
            }
            (Reply::Pending, true)
        });
        self.world.trace(&format!(
            "child {session} turn {request} -> {} prompt {}",
            match &reply {
                Reply::Finished(outcome) => format!("finished {outcome:?}"),
                Reply::Pending => "pending".to_owned(),
            },
            short(prompt)
        ));
        if start {
            self.work(session);
        }
        reply
    }

    fn result(
        &self,
        session: &Arc<str>,
        request: String,
        outcome: Outcome,
    ) -> Result<TurnResult, NanocodeError> {
        match outcome {
            Outcome::Completed(final_message) => {
                if !self.live(|live| {
                    live.completed
                        .insert((Arc::clone(session), request.clone()))
                }) {
                    self.world.wrong(format!(
                        "remote_completion_twice: child {session} turn {request} completed twice in one parent incarnation"
                    ));
                }
                Ok(TurnResult {
                    request_id: Some(request.clone()),
                    turn_id: Some(request),
                    final_message,
                    usage: None,
                    boundary: None,
                })
            }
            Outcome::Cancelled => Err(NanocodeError::TurnCancelled),
        }
    }
}

pub struct Host(pub Arc<SimChildren>);

impl ChildHost for Host {
    fn cancel(
        &self,
        session_id: Arc<str>,
        request_id: Option<String>,
    ) -> BackendFuture<nanocode::backend::Result<()>> {
        let children = Arc::clone(&self.0);
        Box::pin(async move {
            let Some(request) = request_id else {
                return Err(failed("a remote child cancels only an identified turn"));
            };
            let fate = children.link().await;
            if fate == Fate::Unsent {
                return Err(failed("child_cancel never reached the child"));
            }
            let cancelled = children.disk.with(|disk| {
                let child = disk.children.get_mut(&session_id)?;
                let own = child.running.clone()?;
                let parent = child
                    .aliases
                    .get(&own)
                    .cloned()
                    .unwrap_or_else(|| own.clone());
                (own == request || parent == request).then(|| child.finish(Outcome::Cancelled))
            });
            children.world.trace(&format!(
                "child {session_id} cancel {request} -> {cancelled:?}"
            ));
            if cancelled.is_some() {
                children.live(|live| {
                    if let Some((_, worker)) = live.workers.remove(&session_id) {
                        worker.abort();
                    }
                });
                children.pump(&session_id);
            }
            if fate == Fate::Lost {
                return Err(failed("child_cancel's answer was lost"));
            }
            Ok(())
        })
    }

    fn close(&self, session_id: Arc<str>) -> BackendFuture<nanocode::backend::Result<()>> {
        self.0.live(|live| live.open.remove(&session_id));
        Box::pin(async { Ok(()) })
    }

    fn open(&self, child: ChildSpec) -> BackendFuture<nanocode::backend::Result<()>> {
        self.0.world.trace(&format!(
            "child {} open restored={}",
            child.session_id, child.restored
        ));
        self.0
            .live(|live| live.open.insert(Arc::clone(&child.session_id)));
        Box::pin(async { Ok(()) })
    }

    fn pending(
        &self,
        session_id: Arc<str>,
    ) -> BackendFuture<nanocode::backend::Result<Option<(String, Value)>>> {
        let children = Arc::clone(&self.0);
        Box::pin(async move {
            let fate = children.link().await;
            if fate == Fate::Unsent {
                return Err(failed("child_pending never reached the child"));
            }
            let pending = children.disk.with(|disk| {
                let child = disk.children.get(&session_id)?;
                let request = child
                    .running
                    .clone()
                    .filter(|running| child.input(running).is_some())
                    .or_else(|| {
                        child
                            .notices
                            .iter()
                            .rev()
                            .find(|notice| child.input(&notice.request).is_some())
                            .map(|notice| notice.request.clone())
                    })?;
                let input = child.input(&request)?;
                let prompt = Prompt::from(input.prompt.as_str());
                let prompt = match input.revision {
                    Some(revision) => prompt.with_instruction_revision(revision),
                    None => prompt,
                };
                Some((request, prompt))
            });
            children.world.trace(&format!(
                "child {session_id} pending -> {:?}",
                pending.as_ref().map(|(request, _)| request)
            ));
            if fate == Fate::Lost {
                return Err(failed("child_pending's answer was lost"));
            }
            pending
                .map(|(request, prompt)| {
                    serde_json::to_value(prompt)
                        .map(|input| (request, input))
                        .map_err(|error| failed(error.to_string()))
                })
                .transpose()
        })
    }

    fn run(
        &self,
        session_id: Arc<str>,
        request_id: String,
        prompt: Prompt,
    ) -> BackendFuture<nanocode::backend::Result<TurnResult>> {
        let children = Arc::clone(&self.0);
        Box::pin(async move {
            let text = prompt_text(&prompt)?;
            children.live(|live| live.run_calls += 1);
            if !children.live(|live| live.open.contains(&session_id)) {
                return Err(failed(format!("remote child {session_id} is not open")));
            }
            let key = (Arc::clone(&session_id), request_id.clone());
            let (sender, receiver) = oneshot::channel();
            children.live(|live| live.waiters.insert(key.clone(), sender));
            let fate = children.link().await;
            if fate == Fate::Unsent {
                children.live(|live| live.waiters.remove(&key));
                return Err(failed(format!(
                    "remote child {session_id} never received the turn"
                )));
            }
            let revision = prompt.instruction_revision();
            let reply = children.turn(&session_id, &request_id, &text, revision);
            if fate == Fate::Duplicated {
                children.turn(&session_id, &request_id, &text, revision);
            }
            if fate == Fate::Lost {
                children.live(|live| live.waiters.remove(&key));
                return Err(failed(format!(
                    "remote child {session_id}'s answer to the turn was lost"
                )));
            }
            let outcome = match reply {
                Reply::Finished(outcome) => {
                    children.live(|live| live.waiters.remove(&key));
                    outcome
                }
                Reply::Pending => receiver.await.map_err(|_| {
                    failed(format!(
                        "remote child {session_id} turn {request_id} lost its waiter"
                    ))
                })?,
            };
            children.result(&session_id, request_id, outcome)
        })
    }

    fn steer(
        &self,
        session_id: Arc<str>,
        request_id: String,
        id: Option<String>,
        prompt: Prompt,
    ) -> BackendFuture<nanocode::backend::Result<()>> {
        let children = Arc::clone(&self.0);
        Box::pin(async move {
            let text = prompt_text(&prompt)?;
            let fate = children.link().await;
            if fate == Fate::Unsent {
                return Err(NanocodeError::TurnNotSteerable);
            }
            let alive = children.live(|live| live.workers.contains_key(&session_id));
            let steered = alive
                && children.disk.with(|disk| {
                    let Some(child) = disk.children.get_mut(&session_id) else {
                        return false;
                    };
                    if child.running.as_deref() != Some(request_id.as_str()) {
                        return false;
                    }
                    if let Some(id) = &id
                        && !child.steers.insert(id.clone())
                    {
                        return true;
                    }
                    child.received.push(text.clone());
                    true
                });
            children.world.trace(&format!(
                "child {session_id} steer {request_id} id {id:?} -> {steered} {}",
                short(&text)
            ));
            if fate == Fate::Lost || !steered {
                return Err(NanocodeError::TurnNotSteerable);
            }
            Ok(())
        })
    }
}
