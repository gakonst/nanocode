use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{OsStr, OsString},
    io,
    os::{
        fd::OwnedFd,
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock},
};

use nanocode_sync::{Signal, Slot};
use tokio::{
    net::unix::pipe,
    process::{Child, Command},
    sync::{mpsc, oneshot},
};

use super::{output, process, selection::Shell};

mod direct;

pub(crate) use direct::Fifos;

const MAX_JOBS: usize = 997;
const DEPTH: usize = 2;
pub(super) const UNDIFFED: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

type Output = pipe::Receiver;

#[derive(Default)]
pub(super) struct Servers {
    root: OnceLock<Option<PathBuf>>,
    pools: [Pool; 2],
    direct: Arc<std::sync::atomic::AtomicUsize>,
}

pub(super) struct Request<'a> {
    pub(super) script: &'a str,
    pub(super) workdir: &'a Path,
    pub(super) shell: &'a Shell,
    pub(super) login: bool,
    pub(super) environment: &'a [(OsString, OsString)],
    pub(super) secrets: &'a [(OsString, OsString)],
}

impl Drop for Servers {
    fn drop(&mut self) {
        if let Some(Some(root)) = self.root.get() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

impl Servers {
    pub(super) async fn spawn(&self, request: Request<'_>) -> Option<process::SpawnedProcess> {
        if request.shell.name() != "zsh"
            || ["$$", "${$}", "$!", "${!}"]
                .iter()
                .any(|pid| request.script.contains(pid))
            || request.script.contains('\0')
            || !request.workdir.is_dir()
        {
            return None;
        }
        let pool = &self.pools[usize::from(request.login)];
        let root = self.root()?;
        if let Some(words) = direct::words(request.script)
            && let Some(admission) = direct::Admission::new(&self.direct)
        {
            let (offer, offered) = oneshot::channel();
            let (job, pending) = Pending::job(&request, Some(offer));
            pool.dispatcher(root.clone(), request.shell, request.login)
                .send(Note::Job(job))
                .ok()?;
            let Ok(direct) = offered.await else {
                return pending.spawned().await;
            };
            let spawned = direct::spawn(
                admission,
                direct,
                words,
                request.workdir.to_owned(),
                request.environment.to_vec(),
                request.secrets.to_vec(),
                root.clone(),
            )
            .await;
            if spawned.is_some() {
                return spawned;
            }
        }
        let (job, pending) = Pending::job(&request, None);
        pool.dispatcher(root, request.shell, request.login)
            .send(Note::Job(job))
            .ok()?;
        pending.spawned().await
    }

    pub(super) async fn stop(&self) {
        for pool in &self.pools {
            if let Some(dispatcher) = pool.dispatcher.get() {
                let (reply, stopped) = oneshot::channel();
                if dispatcher.send(Note::Stop(reply)).is_ok() {
                    let _ = stopped.await;
                }
            }
        }
    }

    fn root(&self) -> Option<PathBuf> {
        self.root
            .get_or_init(|| {
                let mut random = [0_u8; 8];
                getrandom::fill(&mut random).ok()?;
                let root = std::env::temp_dir().join(format!(
                    "nanocode-shell-{}-{}",
                    std::process::id(),
                    hex(&random)
                ));
                private_dir(&root).ok()?;
                Some(root)
            })
            .clone()
    }
}

struct Pending {
    accepted: oneshot::Receiver<Option<Lease>>,
    exited: oneshot::Receiver<io::Result<i32>>,
    ended: Arc<Signal>,
}

impl Pending {
    fn job(
        request: &Request<'_>,
        direct: Option<oneshot::Sender<Arc<direct::Direct>>>,
    ) -> (Job, Self) {
        let ended = Arc::new(Signal::new());
        let (exit, exited) = oneshot::channel();
        let (reply, accepted) = oneshot::channel();
        let job = Job {
            script: request.script.to_owned(),
            workdir: request.workdir.to_owned(),
            environment: request.environment.to_vec(),
            secrets: request
                .secrets
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            exit,
            ended: Arc::clone(&ended),
            reply,
            direct,
        };
        (
            job,
            Self {
                accepted,
                exited,
                ended,
            },
        )
    }

    async fn spawned(self) -> Option<process::SpawnedProcess> {
        let mut lease = self.accepted.await.ok()??;
        let guard = lease.guard.take()?;
        let child = process::ProcessChild::served(guard, self.exited);
        Some(process::SpawnedProcess {
            process_group: child.process_group(),
            child,
            stdin: None,
            output: process::ProcessOutput::Served(Served {
                lease: Arc::new(lease),
                ended: self.ended,
            }),
        })
    }
}

#[derive(Default)]
struct Pool {
    dispatcher: OnceLock<mpsc::UnboundedSender<Note>>,
}

impl Pool {
    fn dispatcher(
        &self,
        root: PathBuf,
        shell: &Shell,
        login: bool,
    ) -> &mpsc::UnboundedSender<Note> {
        self.dispatcher.get_or_init(|| {
            let (notes, inbox) = mpsc::unbounded_channel();
            let dispatcher = Dispatcher {
                root,
                shell: shell.path().to_owned(),
                login,
                limit: std::thread::available_parallelism().map_or(1, usize::from),
                next: 0,
                queue: VecDeque::new(),
                servers: Vec::new(),
                blocked: None,
                notes: notes.downgrade(),
                probe: None,
                watch: None,
                probed: None,
            };
            tokio::spawn(dispatcher.run(inbox));
            notes
        })
    }
}

struct Probe {
    files: Vec<PathBuf>,
    fingerprint: Fingerprint,
}

struct Job {
    script: String,
    workdir: PathBuf,
    environment: Vec<(OsString, OsString)>,
    secrets: Vec<OsString>,
    exit: oneshot::Sender<io::Result<i32>>,
    ended: Arc<Signal>,
    reply: oneshot::Sender<Option<Lease>>,
    direct: Option<oneshot::Sender<Arc<direct::Direct>>>,
}

enum Note {
    Job(Job),
    Ready(u64, Probe, Arc<direct::Shape>),
    Direct(Fingerprint, Option<Arc<direct::Direct>>),
    Credit(u64),
    Stopped(u64, Option<Fingerprint>),
    Stop(oneshot::Sender<()>),
}

struct Handle {
    id: u64,
    messages: mpsc::UnboundedSender<Message>,
    fingerprint: Option<Fingerprint>,
    credit: usize,
    origin: Arc<direct::Origin>,
}

struct Dispatcher {
    root: PathBuf,
    shell: PathBuf,
    login: bool,
    limit: usize,
    next: u64,
    queue: VecDeque<Job>,
    servers: Vec<Handle>,
    blocked: Option<Fingerprint>,
    notes: mpsc::WeakUnboundedSender<Note>,
    probe: Option<Probed>,
    watch: Option<Watch>,
    probed: Option<Fingerprint>,
}

struct Probed {
    probe: Probe,
    direct: Option<Arc<direct::Direct>>,
}

impl Dispatcher {
    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Note>) {
        let mut notes = Vec::new();
        while let Some(note) = inbox.recv().await {
            notes.push(note);
            while let Ok(note) = inbox.try_recv() {
                notes.push(note);
            }
            if notes.iter().any(|note| matches!(note, Note::Job(_))) {
                self.check();
            }
            for note in notes.drain(..) {
                self.note(note).await;
            }
            self.dispatch();
            if !self.queue.is_empty() {
                self.start(None);
            }
        }
    }

    fn check(&mut self) {
        let Some(Probed { probe, .. }) = self.probe.as_ref() else {
            return;
        };
        if self.watch.as_mut().is_some_and(|watch| !watch.changed())
            || fingerprint(&probe.files) == probe.fingerprint
        {
            return;
        }
        let stale = probe.fingerprint.clone();
        self.probe = None;
        self.servers.retain(|server| {
            let retired = server.fingerprint.as_ref() == Some(&stale);
            if retired {
                let _ = server.messages.send(Message::Retire);
            }
            !retired
        });
    }

    async fn note(&mut self, note: Note) {
        match note {
            Note::Job(mut job) => {
                if let Some(offer) = job.direct.take()
                    && let Some(Probed {
                        direct: Some(direct),
                        ..
                    }) = &self.probe
                {
                    let _ = offer.send(Arc::clone(direct));
                    return;
                }
                if self
                    .servers
                    .iter()
                    .any(|server| server.fingerprint.is_some())
                {
                    self.queue.push_back(job);
                } else {
                    self.start(Some(&job));
                    let _ = job.reply.send(None);
                }
            }
            Note::Ready(id, probe, shape) => {
                if let Some(server) = self.servers.iter_mut().find(|server| server.id == id) {
                    server.fingerprint = Some(probe.fingerprint.clone());
                    server.credit = DEPTH;
                    let origin = Arc::clone(&server.origin);
                    if self
                        .probe
                        .as_ref()
                        .is_none_or(|known| known.direct.is_none())
                        && self.probed.as_ref() != Some(&probe.fingerprint)
                    {
                        self.probed = Some(probe.fingerprint.clone());
                        tokio::spawn(direct::probe(
                            self.notes.clone(),
                            self.root.clone(),
                            origin,
                            shape,
                            probe.fingerprint.clone(),
                        ));
                    }
                    if self
                        .probe
                        .as_ref()
                        .is_none_or(|known| known.probe.files != probe.files)
                    {
                        self.watch = Watch::new(&probe.files);
                    }
                    if let Some(watch) = self.watch.as_mut()
                        && fingerprint(&probe.files) != probe.fingerprint
                    {
                        watch.missed();
                    }
                    let direct = self
                        .probe
                        .take()
                        .filter(|known| known.probe.fingerprint == probe.fingerprint)
                        .and_then(|known| known.direct);
                    self.probe = Some(Probed { probe, direct });
                }
            }
            Note::Direct(fingerprint, direct) => {
                if let Some(known) = self
                    .probe
                    .as_mut()
                    .filter(|known| known.probe.fingerprint == fingerprint)
                {
                    known.direct = direct;
                }
            }
            Note::Credit(id) => {
                if let Some(server) = self.servers.iter_mut().find(|server| server.id == id) {
                    server.credit += 1;
                }
            }
            Note::Stopped(id, blocked) => {
                self.servers.retain(|server| server.id != id);
                if blocked.is_some() {
                    self.blocked = blocked;
                }
                if !self
                    .servers
                    .iter()
                    .any(|server| server.fingerprint.is_some())
                {
                    for job in self.queue.drain(..) {
                        let _ = job.reply.send(None);
                    }
                }
            }
            Note::Stop(reply) => {
                for job in self.queue.drain(..) {
                    let _ = job.reply.send(None);
                }
                self.probe = None;
                for server in self.servers.drain(..) {
                    let (stopped, done) = oneshot::channel();
                    if server.messages.send(Message::Stop(stopped)).is_ok() {
                        let _ = done.await;
                    }
                }
                let _ = reply.send(());
            }
        }
    }

    fn dispatch(&mut self) {
        while !self.queue.is_empty() {
            let Some(server) = self
                .servers
                .iter_mut()
                .filter(|server| server.fingerprint.is_some())
                .max_by_key(|server| server.credit)
                .filter(|server| server.credit > 0)
            else {
                return;
            };
            let Some(job) = self.queue.pop_front() else {
                return;
            };
            server.credit -= 1;
            if let Err(mpsc::error::SendError(Message::Start(job))) =
                server.messages.send(Message::Start(job))
            {
                self.queue.push_front(job);
                return;
            }
        }
    }

    fn start(&mut self, job: Option<&Job>) {
        if self.servers.len() >= self.limit
            || self
                .servers
                .iter()
                .any(|server| server.fingerprint.is_none())
        {
            return;
        }
        let Some(job) = job.or_else(|| self.queue.front()) else {
            return;
        };
        let environment: HashMap<OsString, OsString> = job.environment.iter().cloned().collect();
        let files = startup_files(self.login, &environment, None);
        let blocked = fingerprint(&files);
        if self.blocked.as_ref() == Some(&blocked) {
            return;
        }
        let Some(notes) = self.notes.upgrade() else {
            return;
        };
        self.next += 1;
        let (messages, inbox) = mpsc::unbounded_channel();
        let origin = Arc::new(direct::Origin {
            workdir: job.workdir.clone(),
            environment: environment.clone(),
            listed: job.environment.clone(),
        });
        let launch = Launch {
            id: self.next,
            dir: self
                .root
                .join(format!("{}{}", u8::from(self.login), self.next)),
            shell: self.shell.clone(),
            login: self.login,
            workdir: job.workdir.clone(),
            environment,
            listed: job.environment.clone(),
            probe: Probe {
                files,
                fingerprint: blocked.clone(),
            },
            weak: messages.downgrade(),
            notes: notes.downgrade(),
        };
        self.servers.push(Handle {
            id: self.next,
            messages,
            fingerprint: None,
            credit: 0,
            origin,
        });
        drop(notes);
        tokio::spawn(async move {
            let ready = run(&launch, inbox).await;
            if matches!(ready, Err(Stop::Died)) {
                return;
            }
            if let Err(error) = &ready {
                tracing::warn!(%error, "shell server stopped");
            }
            let blocked = matches!(ready, Err(Stop::Unready(_))).then_some(blocked);
            if let Some(notes) = launch.notes.upgrade() {
                let _ = notes.send(Note::Stopped(launch.id, blocked));
            }
        });
    }
}

struct Ready {
    environment: HashMap<OsString, OsString>,
    listed: Vec<(OsString, OsString)>,
    shape: Arc<direct::Shape>,
    argzero: bool,
    bgnice: bool,
    notify: bool,
}

enum Message {
    Start(Job),
    Release(usize, [Option<Output>; 2]),
    Retire,
    Stop(oneshot::Sender<()>),
}

pub(super) struct Served {
    lease: Arc<Lease>,
    ended: Arc<Signal>,
}

impl Served {
    pub(super) fn drains(
        self,
        chunks: &output::Sender,
        activity: &Arc<nanocode_process_contracts::LastActivity>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        (0..2)
            .map(|stream| {
                tokio::spawn(drain(
                    Arc::clone(&self.lease),
                    stream,
                    Arc::clone(&self.ended),
                    chunks.clone(),
                    Arc::clone(activity),
                ))
            })
            .collect()
    }
}

struct Lease {
    slot: usize,
    guard: Option<process::ProcessGroupGuard>,
    pipes: [Slot<Output>; 2],
    release: mpsc::UnboundedSender<Message>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let pipes = [self.pipes[0].take(), self.pipes[1].take()];
        let _ = self.release.send(Message::Release(self.slot, pipes));
    }
}

async fn drain(
    lease: Arc<Lease>,
    stream: usize,
    ended: Arc<Signal>,
    chunks: output::Sender,
    activity: Arc<nanocode_process_contracts::LastActivity>,
) {
    let Some(pipe) = lease.pipes[stream].take() else {
        return;
    };
    if let Some(pipe) = read_all(pipe, &ended, &chunks, &activity).await {
        lease.pipes[stream].put(pipe);
    }
}

async fn read_all(
    pipe: Output,
    ended: &Signal,
    chunks: &output::Sender,
    activity: &nanocode_process_contracts::LastActivity,
) -> Option<Output> {
    let mut buffer = vec![0_u8; output::READ_BUFFER_LENGTH];
    let mut reading = Reading::Watching { sooner: false };
    loop {
        tokio::task::consume_budget().await;
        let read = match reading {
            Reading::Watching { sooner } => tokio::select! {
                ready = pipe.readable() => ready.and_then(|()| {
                    pipe.try_io(|| rustix::io::read(&pipe, &mut buffer[..]).map_err(io::Error::from))
                }),
                () = ended.wait(), if cfg!(target_os = "macos") => {
                    reading = Reading::Exited;
                    continue;
                }
                () = unreported(sooner) => {
                    reading = Reading::Unreported { found: false };
                    continue;
                }
            },
            Reading::Unreported { found } => match rustix::io::read(&pipe, &mut buffer[..]) {
                Err(rustix::io::Errno::AGAIN) => {
                    reading = Reading::Watching { sooner: found };
                    continue;
                }
                Ok(length) => {
                    reading = Reading::Unreported { found: true };
                    Ok(length)
                }
                read => read.map_err(io::Error::from),
            },
            Reading::Exited => match rustix::io::read(&pipe, &mut buffer[..]) {
                Err(rustix::io::Errno::AGAIN) => {
                    tokio::time::sleep(process::REAP_POLL_INTERVAL).await;
                    continue;
                }
                read => read.map_err(io::Error::from),
            },
        };
        match read {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Ok(0) => return Some(pipe),
            Ok(length) => {
                activity.touch();
                if chunks.send(buffer[..length].to_vec()).await.is_err() {
                    return None;
                }
            }
            Err(error) => {
                let _ = chunks
                    .send(format!("failed to read command output: {error}").into_bytes())
                    .await;
                return None;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Reading {
    Watching { sooner: bool },
    Unreported { found: bool },
    Exited,
}

#[cfg(target_os = "macos")]
const UNREPORTED_READ_INTERVAL: [std::time::Duration; 2] = [
    std::time::Duration::from_millis(50),
    process::REAP_POLL_INTERVAL,
];

#[cfg(target_os = "macos")]
async fn unreported(sooner: bool) {
    tokio::time::sleep(UNREPORTED_READ_INTERVAL[usize::from(sooner)]).await;
}

#[cfg(not(target_os = "macos"))]
async fn unreported(_sooner: bool) {
    std::future::pending::<()>().await;
}

fn prefix(ready: &Ready, job: &Job) -> Option<(Vec<u8>, usize)> {
    let mut prefix = Vec::with_capacity(256 + job.script.len());
    let workdir = job.workdir.as_os_str().as_bytes();
    let logical = job
        .environment
        .iter()
        .find(|(name, _)| name == "PWD")
        .map(|(_, value)| value.as_bytes())
        .filter(|value| value.first() == Some(&b'/'));
    if let Some(logical) = logical {
        prefix.extend_from_slice(b"if [[ ");
        quote(&mut prefix, logical);
        prefix.extend_from_slice(b" -ef ");
        quote(&mut prefix, workdir);
        prefix.extend_from_slice(b" ]]; then \\builtin cd -q -- ");
        quote(&mut prefix, logical);
        prefix.extend_from_slice(b"; else \\builtin cd -qP -- ");
        quote(&mut prefix, workdir);
        prefix.extend_from_slice(b"; fi || \\builtin exit 1; ");
    } else {
        prefix.extend_from_slice(b"\\builtin cd -qP -- ");
        quote(&mut prefix, workdir);
        prefix.extend_from_slice(b" || \\builtin exit 1; ");
    }
    prefix.extend_from_slice(b"OLDPWD=$PWD; ");
    let settable = |name: &OsStr| {
        is_shell_identifier(name.as_bytes()) && !ready.shape.readonly.contains(name.as_bytes())
    };
    let undiffed = |name: &OsStr| UNDIFFED.iter().any(|undiffed| name == *undiffed);
    let same = job.environment == ready.listed;
    for (name, value) in &job.environment {
        let secret = job.secrets.contains(name);
        if undiffed(name) || (!secret && (same || ready.environment.get(name) == Some(value))) {
            continue;
        }
        if !settable(name) {
            return None;
        }
        prefix.extend_from_slice(b"\\builtin export ");
        prefix.extend_from_slice(name.as_bytes());
        prefix.push(b'=');
        quote(&mut prefix, value.as_bytes());
        prefix.extend_from_slice(b"; ");
    }
    let present: HashSet<&OsString> = match same {
        true => HashSet::new(),
        false => job.environment.iter().map(|(name, _)| name).collect(),
    };
    for name in ready.environment.keys().filter(|_| !same) {
        if undiffed(name) || present.contains(name) {
            continue;
        }
        if !settable(name) {
            return None;
        }
        prefix.extend_from_slice(b"\\builtin unset ");
        prefix.extend_from_slice(name.as_bytes());
        prefix.extend_from_slice(b"; ");
    }
    prefix.extend_from_slice(format!("RANDOM={}; SECONDS=0; ", super::random() as u16).as_bytes());
    for (restore, option) in [
        (ready.argzero, "\\builtin setopt function_argzero; "),
        (ready.bgnice, "\\builtin setopt bg_nice; "),
        (!ready.notify, "\\builtin unsetopt notify; "),
    ] {
        if restore {
            prefix.extend_from_slice(option.as_bytes());
        }
    }
    direct(&mut prefix, &job.script);
    let split = prefix.len();
    prefix.extend_from_slice(job.script.as_bytes());
    Some((prefix, split))
}

fn direct(prefix: &mut Vec<u8>, script: &str) {
    let plain = |word: &&str| {
        !word.starts_with('=')
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-./,:+=%".contains(&byte))
    };
    let words = script.split([' ', '\t']).filter(|word| !word.is_empty());
    let Some(command) = words.clone().next() else {
        return;
    };
    if !words.clone().all(|word| plain(&word))
        || command.contains('=')
        || command.starts_with(['-', '%'])
    {
        return;
    }
    prefix.extend_from_slice(
        format!("if (( ! ${{+aliases[{command}]}} && ! ${{+functions[{command}]}}").as_bytes(),
    );
    if let Some((_, suffix)) = command.rsplit_once('.') {
        prefix.extend_from_slice(format!(" && ! ${{+saliases[{suffix}]}}").as_bytes());
    }
    for word in words.clone() {
        prefix.extend_from_slice(format!(" && ! ${{+galiases[{word}]}}").as_bytes());
    }
    if command.contains('/') {
        prefix.extend_from_slice(format!(" )) && [[ -x {command} && ! -d {command} ]]").as_bytes());
    } else {
        prefix.extend_from_slice(
            format!(" && ! ${{+builtins[{command}]}} && ! ${{reswords[(Ie){command}]}} )) && \\builtin whence -p -- {command} >/dev/null").as_bytes(),
        );
    }
    prefix.extend_from_slice(b"; then \\builtin exec");
    for word in words {
        prefix.push(b' ');
        prefix.extend_from_slice(word.as_bytes());
    }
    prefix.extend_from_slice(b"; fi; ");
}

pub(super) fn quote(out: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.extend_from_slice(b"$'");
    for &byte in bytes {
        match byte {
            b'\\' | b'\'' => out.extend_from_slice(&[b'\\', byte]),
            0x20..=0x7e => out.push(byte),
            _ => out.extend_from_slice(&[
                b'\\',
                b'x',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0x0f)],
            ]),
        }
    }
    out.push(b'\'');
}

fn is_shell_identifier(name: &[u8]) -> bool {
    name.first()
        .is_some_and(|first| *first == b'_' || first.is_ascii_alphabetic())
        && name
            .iter()
            .all(|byte| *byte == b'_' || byte.is_ascii_alphanumeric())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Fingerprint(Vec<Option<[i64; 7]>>);

pub(super) fn fingerprint(files: &[PathBuf]) -> Fingerprint {
    Fingerprint(
        files
            .iter()
            .map(|file| {
                std::fs::metadata(file).ok().map(|metadata| {
                    [
                        metadata.dev() as i64,
                        metadata.ino() as i64,
                        metadata.size() as i64,
                        metadata.mtime(),
                        metadata.mtime_nsec(),
                        metadata.ctime(),
                        metadata.ctime_nsec(),
                    ]
                })
            })
            .collect(),
    )
}

#[cfg(target_os = "macos")]
struct Watch {
    kqueue: nix::sys::event::Kqueue,
    _watched: Vec<std::fs::File>,
    pending: bool,
}

#[cfg(target_os = "macos")]
impl Watch {
    fn new(files: &[PathBuf]) -> Option<Self> {
        use nix::sys::event::{EvFlags, EventFilter, FilterFlag, Kqueue};
        use std::os::fd::AsRawFd;
        let kqueue = Kqueue::new().ok()?;
        let mut paths: Vec<PathBuf> = Vec::new();
        for file in files {
            let resolved = std::fs::canonicalize(file).ok();
            for ancestor in [Some(file.as_path()), resolved.as_deref()]
                .into_iter()
                .flatten()
                .flat_map(Path::ancestors)
            {
                if !paths.iter().any(|path| path == ancestor) {
                    paths.push(ancestor.to_owned());
                }
            }
        }
        let flags = FilterFlag::NOTE_DELETE
            | FilterFlag::NOTE_WRITE
            | FilterFlag::NOTE_EXTEND
            | FilterFlag::NOTE_ATTRIB
            | FilterFlag::NOTE_LINK
            | FilterFlag::NOTE_RENAME
            | FilterFlag::NOTE_REVOKE;
        let mut watched = Vec::new();
        let mut changes = Vec::new();
        for path in &paths {
            let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_EVTONLY)
                .open(path)
            else {
                continue;
            };
            let ident = u64::try_from(file.as_raw_fd()).ok()?;
            changes.push(Exits::event(
                ident,
                EventFilter::EVFILT_VNODE,
                EvFlags::EV_ADD | EvFlags::EV_CLEAR,
                flags,
            ));
            watched.push(file);
        }
        Exits::poll(&kqueue, &changes, &mut []).ok()?;
        Some(Self {
            kqueue,
            _watched: watched,
            pending: false,
        })
    }

    const fn missed(&mut self) {
        self.pending = true;
    }

    fn changed(&mut self) -> bool {
        let mut events = [Exits::received(); 16];
        let seen = Exits::poll(&self.kqueue, &[], &mut events).map_or(true, |count| count > 0);
        std::mem::take(&mut self.pending) || seen
    }
}

#[cfg(not(target_os = "macos"))]
enum Watch {}

#[cfg(not(target_os = "macos"))]
impl Watch {
    const fn new(_files: &[PathBuf]) -> Option<Self> {
        None
    }

    const fn missed(&mut self) {
        match *self {}
    }

    const fn changed(&mut self) -> bool {
        match *self {}
    }
}

pub(super) fn startup_files(
    login: bool,
    environment: &HashMap<OsString, OsString>,
    zdotdir: Option<&OsStr>,
) -> Vec<PathBuf> {
    let home = environment.get(OsStr::new("HOME")).map(PathBuf::from);
    let initial = environment
        .get(OsStr::new("ZDOTDIR"))
        .map(PathBuf::from)
        .or(home);
    let mut files = vec![PathBuf::from("/etc/zshenv")];
    files.extend(initial.as_ref().map(|dir| dir.join(".zshenv")));
    if login {
        let profile = zdotdir
            .filter(|zdotdir| !zdotdir.is_empty())
            .map(PathBuf::from)
            .or(initial);
        files.extend(["/etc/zprofile", "/etc/zlogin"].map(PathBuf::from));
        if let Some(dir) = profile {
            files.extend([".zprofile", ".zlogin"].map(|file| dir.join(file)));
        }
        if cfg!(target_os = "macos") {
            files.extend(["/etc/paths", "/etc/paths.d"].map(PathBuf::from));
        }
    }
    files
}

struct Launch {
    id: u64,
    dir: PathBuf,
    shell: PathBuf,
    login: bool,
    workdir: PathBuf,
    environment: HashMap<OsString, OsString>,
    listed: Vec<(OsString, OsString)>,
    probe: Probe,
    weak: mpsc::WeakUnboundedSender<Message>,
    notes: mpsc::WeakUnboundedSender<Note>,
}

enum Stop {
    Launch(io::Error),
    Unready(io::Error),
    Died,
}

impl std::fmt::Display for Stop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Launch(error) => write!(formatter, "before it started: {error}"),
            Self::Unready(error) => write!(formatter, "before it was ready: {error}"),
            Self::Died => formatter.write_str("it died"),
        }
    }
}

fn bootstrap(nonce: &str, dir: &Path) -> OsString {
    let mut script = format!(
        "for __nc_f in /dev/fd/<3->(N:t); do [[ -p /dev/fd/$__nc_f || -S /dev/fd/$__nc_f ]] && \\builtin exec {{__nc_f}}>&-; done\n\\builtin unset __nc_f\n\\builtin zmodload -e zsh/system && __nc_m=0 || __nc_m=1\n\\builtin zmodload zsh/system && \\builtin sysopen -w -o cloexec -u __nc_e /dev/fd/1 || \\builtin exit 1\n(( ! __nc_m )) || \\builtin zmodload -u zsh/system\n\\builtin unset __nc_m\n\\builtin print -r -- {nonce}\n\\builtin print -r -- \"${{ZDOTDIR-}}\"\n\\builtin print -r -- ${{(k)parameters[(R)*readonly*]}}\n\\builtin print -r -- $options[functionargzero] $options[bgnice] $options[notify]\n\\builtin print -r -- ${{(k)aliases[@]:#*[^[:alnum:]_./,:+=%-]*}} ${{(k)functions[@]:#*[^[:alnum:]_./,:+=%-]*}}\n\\builtin print -r -- ${{(k)builtins[@]:#*[^[:alnum:]_./,:+=%-]*}} ${{reswords[@]:#*[^[:alnum:]_./,:+=%-]*}}\n\\builtin print -r -- ${{(k)galiases[@]:#*[^[:alnum:]_./,:+=%-]*}}\n\\builtin print -r -- ${{(k)saliases[@]:#*[^[:alnum:]_./,:+=%-]*}}\n\\builtin hash -r\n\\builtin setopt notify no_bg_nice\n\\builtin setopt monitor 2>/dev/null\n\\builtin print -r -- $options[monitor]\n__nc_d="
    )
    .into_bytes();
    quote(&mut script, dir.as_os_str().as_bytes());
    script.extend_from_slice(
        b"\n\\exec 3>&1 0<\"$__nc_d/q\" 1>/dev/null 2>>|\"$__nc_d/log\" 9</dev/null || \\builtin exit 1\n\
while \\builtin read -r __nc_l; do\n\
if [[ $__nc_l = w* ]]; then\n\
__nc_s=0; \\builtin wait \"${__nc_l#w}\" || __nc_s=$?\n\
\\builtin print -ru3 -- \"x ${__nc_l#w} $__nc_s\"\n\
elif [[ $__nc_l = c* ]]; then\n\
\\builtin wait ${=__nc_l#c} || \\builtin true\n\
else\n\
( \\builtin cd -q -- \"$__nc_d/$__nc_l\" && \\exec 1>|1 2>|2 0<&9 9<&- 3>&- && \\builtin trap \"\\\\builtin print -ru$__nc_e -- 'e $__nc_l '\\$(( \\$? & 255 ))\" EXIT && \\builtin unset __nc_l __nc_d __nc_s __nc_e && \\builtin unsetopt function_argzero && \\builtin source zsh ) &\n\
\\builtin print -ru3 -- \"s $__nc_l $!\"\n\
fi\n\
done\n",
    );
    OsString::from_vec(script)
}

async fn run(launch: &Launch, inbox: mpsc::UnboundedReceiver<Message>) -> Result<(), Stop> {
    private_dir(&launch.dir).map_err(Stop::Launch)?;
    let result = serve(launch, inbox).await;
    let _ = std::fs::remove_dir_all(&launch.dir);
    result
}

async fn serve(launch: &Launch, mut inbox: mpsc::UnboundedReceiver<Message>) -> Result<(), Stop> {
    let (mut child, _keeper, requests, mut events, nonce, _terminal) =
        launch_shell(launch).map_err(Stop::Launch)?;
    let ready = match read_ready(&mut events, &nonce, launch).await {
        Ok(ready) => Exits::new()
            .map(|exits| (ready, exits))
            .map_err(Stop::Launch),
        Err(error) => Err(Stop::Unready(error)),
    };
    let ((ready, probe), mut exits) = match ready {
        Ok(ready) => ready,
        Err(stop) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(stop);
        }
    };
    if let Some(notes) = launch.notes.upgrade() {
        let _ = notes.send(Note::Ready(launch.id, probe, Arc::clone(&ready.shape)));
    }
    let mut owner = Owner {
        id: launch.id,
        ready,
        slots: Slots {
            dir: launch.dir.clone(),
            entries: Vec::new(),
            free: Vec::new(),
        },
        pids: HashMap::new(),
        gone: Vec::new(),
        forget: Vec::new(),
        outbound: Vec::new(),
        requests,
        weak: launch.weak.clone(),
        notes: launch.notes.clone(),
        live: 0,
        life: Life::Serving,
        exit: None,
    };
    let (kill, killed) = oneshot::channel::<()>();
    let (status, mut watcher) = oneshot::channel();
    tokio::spawn(async move {
        let stopped = tokio::select! {
            stopped = child.wait() => stopped,
            _ = killed => {
                let _ = child.start_kill();
                child.wait().await
            }
        };
        let _ = status.send(stopped);
    });
    let stopped = owner
        .serve(&mut watcher, &mut events, &mut inbox, &mut exits)
        .await;
    drop(kill);
    if owner.exit.is_none() {
        let _ = watcher.await;
    }
    owner.life = Life::Stopped;
    let _ = owner.read(&mut events, &exits, true);
    match stopped {
        Ok(reply) => {
            owner.fail();
            if let Some(reply) = reply {
                let _ = reply.send(());
            }
            Ok(())
        }
        Err(error) => {
            owner.died(&error, &launch.dir);
            owner.orphans(&mut events, &mut inbox, &mut exits).await;
            Err(Stop::Died)
        }
    }
}

type Launched = (
    Child,
    pipe::Receiver,
    pipe::Sender,
    Events,
    String,
    Option<OwnedFd>,
);

fn own_terminal() -> Option<(OwnedFd, OwnedFd)> {
    use rustix::fs::{Mode, OFlags, open};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let flags = OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC;
    if open("/dev/tty", flags, Mode::empty()).is_ok() {
        return None;
    }
    #[cfg(target_os = "linux")]
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).ok()?;
    #[cfg(not(target_os = "linux"))]
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).ok()?;
    #[cfg(not(target_os = "linux"))]
    rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC).ok()?;
    grantpt(&master).ok()?;
    unlockpt(&master).ok()?;
    let slave = open(
        ptsname(&master, Vec::new()).ok()?.as_c_str(),
        flags,
        Mode::empty(),
    )
    .ok()?;
    Some((master, slave))
}

fn launch_shell(launch: &Launch) -> io::Result<Launched> {
    let queue = launch.dir.join("q");
    fifo(&queue)?;
    let keeper = pipe::OpenOptions::new().open_receiver(&queue)?;
    let requests = pipe::OpenOptions::new().open_sender(&queue)?;
    let (reader, writer) = io::pipe()?;
    let errors = writer.try_clone()?;
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let nonce = format!("nanocode-shell-{}", hex(&random));
    let (master, stdin) = match own_terminal() {
        Some((master, slave)) => (Some(master), Stdio::from(slave)),
        None => (None, Stdio::null()),
    };
    let mut command = Command::new(&launch.shell);
    command
        .arg(if launch.login { "-lc" } else { "-c" })
        .arg(bootstrap(&nonce, &launch.dir))
        .current_dir(&launch.workdir)
        .env_clear()
        .envs(&launch.environment)
        .stdin(stdin)
        .stdout(Stdio::from(writer))
        .stderr(Stdio::from(errors))
        .kill_on_drop(true)
        .process_group(0);
    let child = command.spawn()?;
    drop(command);
    let events = Events {
        pipe: pipe::Receiver::from_owned_fd(reader.into())?,
        buffer: Vec::new(),
    };
    Ok((child, keeper, requests, events, nonce, master))
}

struct Events {
    pipe: pipe::Receiver,
    buffer: Vec<u8>,
}

impl Events {
    fn fill(&mut self, force: bool) -> io::Result<bool> {
        let mut chunk = [0_u8; output::READ_BUFFER_LENGTH];
        if force {
            loop {
                match rustix::io::read(&self.pipe, &mut chunk) {
                    Ok(0) => return Ok(false),
                    Ok(read) => self.buffer.extend_from_slice(&chunk[..read]),
                    Err(rustix::io::Errno::INTR) => {}
                    Err(rustix::io::Errno::AGAIN) => break,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        loop {
            match self.pipe.try_read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(read) => self.buffer.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(true),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn take(&mut self) -> Vec<u8> {
        let end = self
            .buffer
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |end| end + 1);
        self.buffer.drain(..end).collect()
    }

    async fn line(&mut self) -> io::Result<Vec<u8>> {
        let mut open = true;
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
                line.pop();
                return Ok(line);
            }
            if !open {
                return Err(io::Error::other("the shell stopped while it started"));
            }
            self.pipe.readable().await?;
            open = self.fill(false)?;
        }
    }
}

async fn read_ready(
    events: &mut Events,
    nonce: &str,
    launch: &Launch,
) -> io::Result<(Ready, Probe)> {
    if events.line().await? != nonce.as_bytes() {
        return Err(io::Error::other(
            "the shell printed output while it started",
        ));
    }
    let zdotdir = OsString::from_vec(events.line().await?);
    let readonly = events
        .line()
        .await?
        .split(u8::is_ascii_whitespace)
        .filter(|name| !name.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    let options: Vec<bool> = events
        .line()
        .await?
        .split(u8::is_ascii_whitespace)
        .map(|value| value == b"on")
        .collect();
    let [argzero, bgnice, notify] = options[..] else {
        return Err(io::Error::other("the shell reported unexpected options"));
    };
    let mut names = Vec::with_capacity(4);
    for _ in 0..4 {
        let line = events.line().await?;
        names.push(
            line.split(u8::is_ascii_whitespace)
                .filter(|name| !name.is_empty())
                .map(<[u8]>::to_vec)
                .collect::<HashSet<_>>(),
        );
    }
    let [commands, builtins, global, suffixes] = <[_; 4]>::try_from(names)
        .map_err(|_| io::Error::other("the shell reported unexpected names"))?;
    if events.line().await? != b"on" {
        return Err(io::Error::other("zsh has no job control"));
    }
    let files = startup_files(launch.login, &launch.environment, Some(&zdotdir));
    let probe = Probe {
        fingerprint: if files == launch.probe.files {
            launch.probe.fingerprint.clone()
        } else {
            fingerprint(&files)
        },
        files,
    };
    let ready = Ready {
        environment: launch.environment.clone(),
        listed: launch.listed.clone(),
        shape: Arc::new(direct::Shape {
            readonly,
            commands,
            builtins,
            global,
            suffixes,
        }),
        argzero,
        bgnice,
        notify,
    };
    Ok((ready, probe))
}

fn private_dir(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

fn fifo(path: &Path) -> io::Result<()> {
    use nix::sys::stat::Mode;
    Ok(nix::unistd::mkfifo(path, Mode::S_IRUSR | Mode::S_IWUSR)?)
}

struct Slots {
    dir: PathBuf,
    entries: Vec<Option<Entry>>,
    free: Vec<usize>,
}

struct Entry {
    pipes: [Option<Output>; 2],
    script: std::fs::File,
    length: usize,
    busy: Option<Busy>,
}

struct Busy {
    secret: bool,
    stage: Stage,
    owed: Option<Owed>,
    ended: Arc<Signal>,
    released: Option<[Option<Output>; 2]>,
    reported: Option<i32>,
}

enum Stage {
    Forking {
        reply: oneshot::Sender<Option<Lease>>,
        release: mpsc::UnboundedSender<Message>,
    },
    Running,
    Unreported,
    Waited,
}

struct Owed {
    exit: oneshot::Sender<io::Result<i32>>,
    keeper: [std::fs::File; 2],
}

impl Entry {
    fn write(&mut self, text: &[u8], split: usize) -> io::Result<()> {
        if self.length.saturating_sub(text.len()) > text.len() {
            self.script.set_len(0)?;
            self.length = 0;
        }
        let padding = self.length.saturating_sub(text.len());
        let mut padded = Vec::with_capacity(text.len() + padding);
        padded.extend_from_slice(&text[..split]);
        padded.resize(split + padding, b' ');
        padded.extend_from_slice(&text[split..]);
        std::os::unix::fs::FileExt::write_all_at(&self.script, &padded, 0)?;
        self.length = self.length.max(padded.len());
        Ok(())
    }

    fn erase(&self) -> io::Result<()> {
        std::os::unix::fs::FileExt::write_all_at(&self.script, &vec![b' '; self.length], 0)
    }
}

impl Busy {
    fn end(&mut self, status: io::Result<i32>) {
        let owed = self.owed.take();
        let exit = owed.map(|Owed { exit, keeper }| {
            drop(keeper);
            exit
        });
        self.ended.fire();
        if let Some(exit) = exit {
            let _ = exit.send(status);
        }
    }

    fn abandon(&mut self) {
        if let Stage::Forking { .. } = self.stage
            && let Stage::Forking { reply, .. } = std::mem::replace(&mut self.stage, Stage::Running)
        {
            let _ = reply.send(None);
        }
    }
}

impl Slots {
    fn path(&self, slot: usize) -> PathBuf {
        self.dir.join(slot.to_string())
    }

    fn lease(&mut self) -> io::Result<usize> {
        if let Some(slot) = self.free.pop() {
            return Ok(slot);
        }
        let slot = self.entries.len();
        let path = self.path(slot);
        private_dir(&path)?;
        let open = |name| {
            fifo(&path.join(name))?;
            pipe::OpenOptions::new().open_receiver(path.join(name))
        };
        let pipes = [Some(open("1")?), Some(open("2")?)];
        let script = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path.join("zsh"))?;
        self.entries.push(Some(Entry {
            pipes,
            script,
            length: 0,
            busy: None,
        }));
        Ok(slot)
    }

    fn keeper(&self, slot: usize) -> io::Result<[std::fs::File; 2]> {
        let path = self.path(slot);
        let open = |name| {
            std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(path.join(name))
        };
        Ok([open("1")?, open("2")?])
    }

    fn reregister(&mut self, slot: usize) -> io::Result<()> {
        let entry = self
            .entry(slot)
            .ok_or_else(|| io::Error::other("the slot is gone"))?;
        for pipe in &mut entry.pipes {
            if let Some(reader) = pipe.take() {
                *pipe = Some(pipe::Receiver::from_owned_fd_unchecked(
                    reader.into_nonblocking_fd()?,
                )?);
            }
        }
        Ok(())
    }

    fn retire(&mut self, slot: usize) {
        if let Some(entry) = self.entries.get_mut(slot) {
            *entry = None;
            let _ = std::fs::remove_dir_all(self.path(slot));
        }
    }

    fn entry(&mut self, slot: usize) -> Option<&mut Entry> {
        self.entries.get_mut(slot).and_then(Option::as_mut)
    }

    fn busy(&mut self, slot: usize) -> Option<&mut Busy> {
        self.entry(slot).and_then(|entry| entry.busy.as_mut())
    }

    fn waiting(&self) -> bool {
        self.entries
            .iter()
            .flatten()
            .filter_map(|entry| entry.busy.as_ref())
            .any(|busy| busy.owed.is_some())
    }

    fn settle(&mut self, slot: usize) {
        let Some(entry) = self.entry(slot) else {
            return;
        };
        if entry
            .busy
            .as_ref()
            .is_none_or(|busy| !matches!(busy.stage, Stage::Waited) || busy.released.is_none())
        {
            return;
        }
        if let Some(Busy {
            released: Some([Some(stdout), Some(stderr)]),
            ..
        }) = entry.busy.take()
        {
            entry.pipes = [Some(stdout), Some(stderr)];
            self.free.push(slot);
        } else {
            self.retire(slot);
        }
    }
}

enum Event {
    Message(Option<Message>),
    Readable(io::Result<()>),
    Exited(io::Result<()>),
    Child(io::Result<std::process::ExitStatus>),
    Writable(io::Result<()>),
}

struct Owner {
    id: u64,
    ready: Ready,
    slots: Slots,
    pids: HashMap<u32, usize>,
    gone: Vec<u32>,
    forget: Vec<u32>,
    outbound: Vec<u8>,
    requests: pipe::Sender,
    weak: mpsc::WeakUnboundedSender<Message>,
    notes: mpsc::WeakUnboundedSender<Note>,
    live: usize,
    life: Life,
    exit: Option<String>,
}

enum Life {
    Serving,
    Retiring,
    Stopped,
}

impl Owner {
    async fn serve(
        &mut self,
        watcher: &mut oneshot::Receiver<io::Result<std::process::ExitStatus>>,
        events: &mut Events,
        inbox: &mut mpsc::UnboundedReceiver<Message>,
        exits: &mut Exits,
    ) -> io::Result<Option<oneshot::Sender<()>>> {
        let mut exited = Vec::new();
        loop {
            if matches!(self.life, Life::Retiring) && self.live == 0 {
                return Ok(None);
            }
            let event = tokio::select! {
                message = inbox.recv() => Event::Message(message),
                ready = events.pipe.readable() => Event::Readable(ready),
                ready = exits.next(&mut exited) => Event::Exited(ready),
                status = &mut *watcher, if self.exit.is_none() => Event::Child(status.unwrap_or_else(|_| Err(io::Error::other("the shell server's watcher stopped")))),
                writable = self.requests.writable(), if !self.outbound.is_empty() => Event::Writable(writable),
            };
            match event {
                Event::Message(Some(Message::Start(job))) => self.start(job),
                Event::Message(Some(Message::Release(slot, pipes))) => {
                    if let Some(busy) = self.slots.busy(slot) {
                        busy.released = Some(pipes);
                    }
                    self.slots.settle(slot);
                }
                Event::Message(Some(Message::Retire)) => self.life = Life::Retiring,
                Event::Message(Some(Message::Stop(reply))) => return Ok(Some(reply)),
                Event::Message(None) => return Ok(None),
                Event::Readable(ready) => {
                    ready?;
                    if !self.read(events, exits, false)? {
                        return Err(io::Error::other("the shell server closed its output"));
                    }
                }
                Event::Exited(ready) => {
                    ready?;
                    if !self.exited(&mut exited, events, exits)? {
                        return Err(io::Error::other("the shell server closed its output"));
                    }
                }
                Event::Child(status) => {
                    let status =
                        status.map_or_else(|error| error.to_string(), |status| status.to_string());
                    let error = io::Error::other(format!("the shell server exited: {status}"));
                    self.exit = Some(status);
                    return Err(error);
                }
                Event::Writable(writable) => writable?,
            }
            self.flush()?;
        }
    }

    async fn orphans(
        &mut self,
        events: &mut Events,
        inbox: &mut mpsc::UnboundedReceiver<Message>,
        exits: &mut Exits,
    ) {
        for entry in self.slots.entries.iter_mut().flatten() {
            let Some(busy) = entry.busy.as_mut() else {
                continue;
            };
            if let Stage::Forking { .. } = busy.stage {
                busy.abandon();
                entry.busy = None;
            } else if let Stage::Unreported = busy.stage {
                busy.end(Err(io::Error::other("the shell server stopped")));
            }
        }
        let mut exited = Vec::new();
        let mut listening = true;
        let mut open = true;
        while self.slots.waiting() {
            tokio::select! {
                message = inbox.recv(), if listening => match message {
                    Some(Message::Start(job)) => {
                        let _ = job.reply.send(None);
                    }
                    Some(Message::Release(slot, pipes)) => {
                        if let Some(busy) = self.slots.busy(slot) {
                            busy.released = Some(pipes);
                        }
                    }
                    Some(Message::Retire) => {}
                    Some(Message::Stop(reply)) => {
                        self.fail();
                        let _ = reply.send(());
                        return;
                    }
                    None => listening = false,
                },
                ready = events.pipe.readable(), if open => {
                    open = ready.is_ok() && self.read(events, exits, false).unwrap_or(false);
                }
                ready = exits.next(&mut exited) => {
                    if ready.is_err() {
                        break;
                    }
                    let _ = self.exited(&mut exited, events, exits);
                }
            }
        }
        self.fail();
    }

    fn died(&self, error: &io::Error, dir: &Path) {
        let log = std::fs::read_to_string(dir.join("log")).unwrap_or_default();
        let zsh = log
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default();
        tracing::warn!(%error, zsh, "shell server stopped");
        if let Some(notes) = self.notes.upgrade() {
            let _ = notes.send(Note::Stopped(self.id, None));
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.outbound.is_empty() && !self.forget.is_empty() {
            self.outbound.push(b'c');
            for pid in self.forget.drain(..) {
                self.outbound
                    .extend_from_slice(format!(" {pid}").as_bytes());
            }
            self.outbound.push(b'\n');
        }
        while !self.outbound.is_empty() {
            match self.requests.try_write(&self.outbound) {
                Ok(written) => drop(self.outbound.drain(..written)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn credit(&self) {
        if let Some(notes) = self.notes.upgrade() {
            let _ = notes.send(Note::Credit(self.id));
        }
    }

    fn start(&mut self, job: Job) {
        let release = self
            .weak
            .upgrade()
            .filter(|_| matches!(self.life, Life::Serving) && self.live < MAX_JOBS);
        let content = release.as_ref().and_then(|_| prefix(&self.ready, &job));
        let started = match (release, content) {
            (Some(release), Some((text, split))) => self.prepare(job, &text, split, release),
            _ => Err((job.reply, None)),
        };
        if let Err((reply, error)) = started {
            if let Some(error) = error {
                tracing::debug!(%error, "the shell server could not prepare a command");
            }
            let _ = reply.send(None);
            self.credit();
        }
    }

    fn prepare(
        &mut self,
        job: Job,
        text: &[u8],
        split: usize,
        release: mpsc::UnboundedSender<Message>,
    ) -> Result<(), (oneshot::Sender<Option<Lease>>, Option<io::Error>)> {
        let secret = !job.secrets.is_empty();
        let Job {
            exit, ended, reply, ..
        } = job;
        if reply.is_closed() {
            return Err((reply, None));
        }
        let slot = match self.slots.lease() {
            Ok(slot) => slot,
            Err(error) => return Err((reply, Some(error))),
        };
        let keeper = match self.slots.keeper(slot) {
            Ok(keeper) => keeper,
            Err(error) => {
                self.slots.free.push(slot);
                return Err((reply, Some(error)));
            }
        };
        if let Err(error) = self.slots.reregister(slot) {
            self.slots.retire(slot);
            return Err((reply, Some(error)));
        }
        let Some(entry) = self.slots.entry(slot) else {
            return Err((reply, None));
        };
        if let Err(error) = entry.write(text, split) {
            self.slots.free.push(slot);
            return Err((reply, Some(error)));
        }
        entry.busy = Some(Busy {
            secret,
            stage: Stage::Forking { reply, release },
            owed: Some(Owed { exit, keeper }),
            ended,
            released: None,
            reported: None,
        });
        self.live += 1;
        self.outbound
            .extend_from_slice(format!("{slot}\n").as_bytes());
        Ok(())
    }

    fn read(&mut self, events: &mut Events, exits: &Exits, force: bool) -> io::Result<bool> {
        let mut open = self.handle(events, exits, force)?;
        while !self.gone.is_empty() {
            let gone = std::mem::take(&mut self.gone);
            open = self.handle(events, exits, true)? && open;
            for pid in gone {
                self.reap(pid);
            }
        }
        Ok(open)
    }

    fn handle(&mut self, events: &mut Events, exits: &Exits, force: bool) -> io::Result<bool> {
        let open = events.fill(force)?;
        for line in events.take().split(|byte| *byte == b'\n') {
            self.event(line, exits);
        }
        Ok(open)
    }

    fn exited(
        &mut self,
        exited: &mut Vec<u32>,
        events: &mut Events,
        exits: &Exits,
    ) -> io::Result<bool> {
        let open = self.read(events, exits, true)?;
        for pid in exited.drain(..) {
            self.reap(pid);
        }
        Ok(open)
    }

    fn reap(&mut self, pid: u32) {
        let Some(&slot) = self.pids.get(&pid) else {
            return;
        };
        let Some(entry) = self.slots.entry(slot) else {
            return;
        };
        let Some(busy) = entry.busy.as_mut() else {
            return;
        };
        match busy.reported {
            Some(status) => {
                busy.end(Ok(status));
                busy.stage = Stage::Waited;
                if busy.secret {
                    let _ = entry.erase();
                }
                self.pids.remove(&pid);
                self.forget.push(pid);
                self.live = self.live.saturating_sub(1);
                self.slots.settle(slot);
            }
            None if !matches!(self.life, Life::Stopped) => {
                busy.stage = Stage::Unreported;
                self.outbound
                    .extend_from_slice(format!("w{pid}\n").as_bytes());
            }
            None => {
                busy.end(Err(io::Error::other("the shell server stopped")));
                self.pids.remove(&pid);
            }
        }
    }

    fn event(&mut self, line: &[u8], exits: &Exits) {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        let mut fields = line.split_ascii_whitespace();
        let (Some(kind), Some(Ok(first)), Some(Ok(second))) = (
            fields.next(),
            fields.next().map(str::parse::<u32>),
            fields.next().map(str::parse::<i32>),
        ) else {
            return;
        };
        let Ok(slot) = usize::try_from(first) else {
            return;
        };
        match kind {
            "s" => self.started(slot, second, exits),
            "e" => {
                if let Some(busy) = self.slots.busy(slot) {
                    busy.reported = Some(second);
                }
            }
            "x" => self.waited(first, second),
            _ => {}
        }
    }

    fn started(&mut self, slot: usize, pid: i32, exits: &Exits) {
        self.credit();
        let Ok(pid) = u32::try_from(pid) else {
            return;
        };
        let Some(entry) = self.slots.entry(slot) else {
            return;
        };
        let Some(busy) = entry.busy.as_mut() else {
            return;
        };
        let stage = std::mem::replace(&mut busy.stage, Stage::Running);
        let owned = match (stage, std::mem::take(&mut entry.pipes)) {
            (Stage::Forking { reply, release }, [Some(stdout), Some(stderr)]) => reply
                .send(Some(Lease {
                    slot,
                    guard: Some(process::ProcessGroupGuard::new(pid)),
                    pipes: [Slot::new(Some(stdout)), Slot::new(Some(stderr))],
                    release,
                }))
                .is_ok(),
            (_, pipes) => {
                entry.pipes = pipes;
                false
            }
        };
        if !owned {
            busy.end(Err(io::Error::other("the command was cancelled")));
            drop(process::ProcessGroupGuard::new(pid));
        }
        self.pids.insert(pid, slot);
        if exits.watch(pid) {
            self.gone.push(pid);
        }
    }

    fn waited(&mut self, pid: u32, status: i32) {
        let Some(slot) = self.pids.remove(&pid) else {
            return;
        };
        if let Some(entry) = self.slots.entry(slot)
            && let Some(busy) = entry.busy.as_mut()
        {
            busy.end(Ok(status));
            busy.stage = Stage::Waited;
            if busy.secret {
                let _ = entry.erase();
            }
            self.live = self.live.saturating_sub(1);
        }
        self.slots.settle(slot);
    }

    fn fail(&mut self) {
        for entry in self.slots.entries.iter_mut().flatten() {
            if let Some(busy) = entry.busy.as_mut() {
                busy.abandon();
                busy.end(Err(io::Error::other("the shell server stopped")));
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct Exits {
    kqueue: nix::sys::event::Kqueue,
    ready: pipe::Receiver,
}

#[cfg(target_os = "macos")]
impl Exits {
    fn new() -> io::Result<Self> {
        use std::os::fd::AsFd;
        let kqueue = nix::sys::event::Kqueue::new()?;
        let ready = pipe::Receiver::from_owned_fd_unchecked(kqueue.as_fd().try_clone_to_owned()?)?;
        Ok(Self { kqueue, ready })
    }

    fn event(
        ident: u64,
        filter: nix::sys::event::EventFilter,
        flags: nix::sys::event::EvFlags,
        fflags: nix::sys::event::FilterFlag,
    ) -> nix::sys::event::KEvent64 {
        nix::sys::event::KEvent64::new(ident, filter, flags, fflags, 0, 0, [0; 2])
    }

    fn received() -> nix::sys::event::KEvent64 {
        use nix::sys::event::{EvFlags, EventFilter, FilterFlag};
        Self::event(
            0,
            EventFilter::EVFILT_PROC,
            EvFlags::empty(),
            FilterFlag::empty(),
        )
    }

    fn poll(
        kqueue: &nix::sys::event::Kqueue,
        changes: &[nix::sys::event::KEvent64],
        events: &mut [nix::sys::event::KEvent64],
    ) -> nix::Result<usize> {
        use nix::sys::event::Kevent64Flags;
        kqueue.kevent64(changes, events, Kevent64Flags::KEVENT_FLAG_IMMEDIATE, None)
    }

    fn watch(&self, pid: u32) -> bool {
        use nix::sys::event::{EvFlags, EventFilter, FilterFlag};
        let event = Self::event(
            u64::from(pid),
            EventFilter::EVFILT_PROC,
            EvFlags::EV_ADD | EvFlags::EV_ONESHOT,
            FilterFlag::NOTE_EXIT,
        );
        Self::poll(&self.kqueue, &[event], &mut []).is_err()
    }

    async fn next(&mut self, exited: &mut Vec<u32>) -> io::Result<()> {
        use nix::sys::event::FilterFlag;
        self.ready.readable().await?;
        let mut events = [Self::received(); 64];
        let drained = self.ready.try_io(|| {
            let count = Self::poll(&self.kqueue, &[], &mut events)?;
            exited.extend(
                events[..count]
                    .iter()
                    .filter(|event| event.fflags().contains(FilterFlag::NOTE_EXIT))
                    .filter_map(|event| u32::try_from(event.ident()).ok()),
            );
            if count < events.len() {
                Err(io::ErrorKind::WouldBlock.into())
            } else {
                Ok(())
            }
        });
        match drained {
            Err(error) if error.kind() != io::ErrorKind::WouldBlock => Err(error),
            _ => Ok(()),
        }
    }
}

#[cfg(target_os = "linux")]
struct Exits {
    sender: mpsc::UnboundedSender<u32>,
    receiver: mpsc::UnboundedReceiver<u32>,
}

#[cfg(target_os = "linux")]
impl Exits {
    fn new() -> io::Result<Self> {
        let (sender, receiver) = mpsc::unbounded_channel();
        Ok(Self { sender, receiver })
    }

    fn watch(&self, pid: u32) -> bool {
        let descriptor = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .and_then(|process| {
                rustix::process::pidfd_open(process, rustix::process::PidfdFlags::NONBLOCK).ok()
            })
            .and_then(|descriptor| pipe::Receiver::from_owned_fd_unchecked(descriptor).ok());
        let Some(descriptor) = descriptor else {
            return true;
        };
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let _ = descriptor.readable().await;
            let _ = sender.send(pid);
        });
        false
    }

    async fn next(&mut self, exited: &mut Vec<u32>) -> io::Result<()> {
        if let Some(pid) = self.receiver.recv().await {
            exited.push(pid);
        }
        while let Ok(pid) = self.receiver.try_recv() {
            exited.push(pid);
        }
        Ok(())
    }
}
