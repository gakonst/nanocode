use crate::{
    answers::{self, Answers},
    command::Commands,
    fetch::{Fetch, Network, Stepping},
    file::{Capture, Source},
    fs::FileSystem,
    host::{Host, Vfs},
    machine::{Identity, Machine},
};
use bytes::Bytes;
use psh_engine::{HostFile, ShellConfig, util::UtilEntry};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::BTreeSet,
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::OnceLock,
    task::{Context, Poll, Waker},
};

fn utilities() -> &'static [UtilEntry] {
    static ALL: OnceLock<Vec<UtilEntry>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = psh_utils::REGISTRY.to_vec();
        all.extend_from_slice(psh_utils::HOST_FACTS);
        all.extend_from_slice(&crate::net::UTILITIES);
        all
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub fuel: u64,
    pub output: u64,
    pub file: u64,
    pub operations: u64,
    pub depth: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Fuel,
    Missing,
    Operations,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    Exited(i32),
    Cancelled,
    Refused(Limit),
    Unanswered(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub written: BTreeSet<Vec<u8>>,
    pub removed: BTreeSet<Vec<u8>>,
    pub made: BTreeSet<Vec<u8>>,
    pub linked: BTreeSet<Vec<u8>>,
    pub modes: BTreeSet<Vec<u8>>,
    pub moved: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Changes {
    pub(crate) fn wrote(&mut self, path: Vec<u8>) {
        self.removed.remove(&path);
        self.written.insert(path);
    }

    pub(crate) fn removed(&mut self, path: Vec<u8>) {
        self.written.remove(&path);
        self.made.remove(&path);
        self.linked.remove(&path);
        self.modes.remove(&path);
        self.removed.insert(path);
    }

    pub(crate) fn moved(&mut self, from: Vec<u8>, to: Vec<u8>) {
        let mut carried = false;
        for (set, creates) in [
            (&mut self.written, true),
            (&mut self.made, true),
            (&mut self.linked, true),
            (&mut self.modes, false),
        ] {
            let under: Vec<Vec<u8>> = set
                .iter()
                .filter(|path| within(path, &from))
                .cloned()
                .collect();
            for path in under {
                set.remove(&path);
                carried |= creates && path == from;
                let mut placed = to.clone();
                placed.extend_from_slice(&path[from.len()..]);
                set.insert(placed);
            }
        }
        self.removed.remove(&to);
        if !carried {
            self.moved.push((from, to));
        }
    }
}

fn within(path: &[u8], dir: &[u8]) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
}

#[derive(Debug)]
pub struct Ran {
    pub ended: Ended,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub fuel: u64,
    pub operations: u64,
    pub missing: Vec<Vec<u8>>,
    pub changes: Changes,
}

#[derive(Debug)]
pub enum Step {
    Done(Box<Ran>),
    Blocked,
    Yield,
    Idle,
}

pub struct Script {
    pub source: Vec<u8>,
    pub name: Vec<u8>,
    pub args: Vec<Vec<u8>>,
    pub options: Vec<Vec<u8>>,
    pub stdin: Option<Bytes>,
}

pub struct Config {
    pub fs: Rc<dyn FileSystem>,
    pub commands: Rc<dyn Commands>,
    pub machine: Rc<dyn Machine>,
    pub identity: Identity,
    pub limits: Limits,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub cwd: Vec<u8>,
    pub without: Vec<Vec<u8>>,
    pub strict: bool,
}

pub struct Shell {
    engine: psh_engine::Shell,
    vfs: Rc<Vfs>,
    stdin: Rc<Source>,
    stdout: Rc<Capture>,
    stderr: Rc<Capture>,
    spent: u64,
    cancelled: bool,
    network: Option<Network>,
}

impl Shell {
    pub fn new(config: Config) -> Self {
        Self::build(config, None)
    }

    pub fn with_answers(config: Config, answers: Answers) -> Self {
        Self::build(config, Some(answers))
    }

    fn build(config: Config, answers: Option<Answers>) -> Self {
        let mut without = config.without;
        if let Some(answers) = &answers {
            let unanswered = utilities()
                .iter()
                .flat_map(|utility| utility.names)
                .filter(|name| !answers.runs(name))
                .map(|name| name.as_bytes().to_vec());
            without.extend(unanswered);
        }
        let vfs = Rc::new(Vfs {
            fs: config.fs,
            commands: config.commands,
            machine: config.machine,
            identity: config.identity,
            limits: config.limits,
            strict: config.strict,
            operations: Cell::new(0),
            changes: RefCell::new(Changes::default()),
            missing: RefCell::new(Vec::new()),
            refused: Cell::new(None),
            answers,
            unanswered: RefCell::new(None),
            control: OnceCell::new(),
            spawned: Cell::new(0),
        });
        let stdin = Rc::new(Source::new(vfs.clone()));
        let stdout = Rc::new(Capture::new(config.limits.output));
        let stderr = Rc::new(Capture::new(config.limits.output));
        let mut engine = ShellConfig::new(Rc::new(Host(vfs.clone())));
        engine.env = config.env;
        engine.cwd = config.cwd;
        engine.stdin = Some(stdin.clone() as Rc<dyn HostFile>);
        engine.stdout = Some(stdout.clone() as Rc<dyn HostFile>);
        engine.stderr = Some(stderr.clone() as Rc<dyn HostFile>);
        engine.max_depth = config.limits.depth;
        engine.utilities = psh_utils::policy(true);
        engine.utilities.entries = utilities();
        engine.utilities.off_names = without;
        let engine = psh_engine::Shell::new(engine);
        let _ = vfs.control.set(engine.control());
        Self {
            engine,
            vfs,
            stdin,
            stdout,
            stderr,
            spent: 0,
            cancelled: false,
            network: None,
        }
    }

    pub fn set_fetch(&mut self, fetch: Rc<dyn Fetch>) {
        self.network = Some(Network {
            fetch,
            limit: self.vfs.limits.file,
        });
    }

    pub fn start(&mut self, script: Script) {
        self.stdin.set(script.stdin);
        self.spent = self.engine.spent();
        self.cancelled = false;
        self.vfs.operations.set(0);
        self.vfs.refused.set(None);
        self.vfs.unanswered.take();
        self.vfs.missing.borrow_mut().clear();
        self.vfs.changes.take();
        self.stdout.take();
        self.stderr.take();
        let admitted = self
            .vfs
            .answers
            .as_ref()
            .map_or(Ok(()), |answers| answers::admit(&script.source, answers));
        let source = match admitted {
            Ok(()) => script.source,
            Err(what) => {
                self.vfs.unanswered.replace(Some(what));
                Vec::new()
            }
        };
        self.engine.start(psh_engine::Script {
            source,
            arg0: script.name,
            args: script.args,
            dash_c: true,
            options: script.options,
            stdin: false,
        });
    }

    pub fn renew(&mut self) {
        self.engine.renew();
    }

    pub fn set_waker(&self, waker: Waker) {
        self.engine.set_waker(waker);
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.engine.control().cancel();
    }

    pub fn step(&mut self, budget: u32) -> Step {
        let left = self.vfs.limits.fuel.saturating_sub(self.fuel());
        let slice = u32::try_from(left).map_or(budget, |left| left.min(budget));
        if slice == 0 && budget != 0 {
            self.vfs.refuse(Limit::Fuel);
        }
        let _stepping = Stepping::with(self.network.clone());
        match self.engine.step(slice) {
            psh_engine::Step::Done(status) => Step::Done(Box::new(self.ran(status))),
            psh_engine::Step::Blocked => Step::Blocked,
            psh_engine::Step::Yield => Step::Yield,
            psh_engine::Step::Idle => Step::Idle,
        }
    }

    pub fn run(&mut self, script: Script, budget: u32) -> Run<'_> {
        self.start(script);
        Run {
            shell: self,
            budget,
        }
    }

    fn fuel(&self) -> u64 {
        self.engine.spent() - self.spent
    }

    fn ran(&mut self, status: i32) -> Ran {
        let (stdout, cut_out) = self.stdout.take();
        let (stderr, cut_err) = self.stderr.take();
        let ended = match (self.vfs.unanswered.take(), self.vfs.refused.take()) {
            (Some(what), _) => Ended::Unanswered(what),
            (None, Some(limit)) => Ended::Refused(limit),
            (None, None) if self.cancelled => Ended::Cancelled,
            (None, None) => Ended::Exited(status),
        };
        Ran {
            ended,
            stdout,
            stderr,
            truncated: cut_out || cut_err,
            fuel: self.fuel(),
            operations: self.vfs.operations.get(),
            missing: self.vfs.missing.take(),
            changes: self.vfs.changes.take(),
        }
    }
}

pub struct Run<'a> {
    shell: &'a mut Shell,
    budget: u32,
}

impl Future for Run<'_> {
    type Output = Ran;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Ran> {
        let run = self.get_mut();
        run.shell.set_waker(cx.waker().clone());
        match run.shell.step(run.budget) {
            Step::Done(ran) => Poll::Ready(*ran),
            Step::Blocked => Poll::Pending,
            Step::Yield => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Step::Idle => panic!("a finished run was polled again"),
        }
    }
}
