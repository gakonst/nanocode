use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{OsStr, OsString},
    io,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
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

const MAX_JOBS: usize = 997;
const DEPTH: usize = 2;
const UNDIFFED: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

type Output = pipe::Receiver;

#[derive(Default)]
pub(super) struct Servers {
    root: OnceLock<Option<PathBuf>>,
    pools: [Pool; 2],
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
        };
        pool.dispatcher(self.root()?, request.shell, request.login)
            .send(Note::Job(job))
            .ok()?;
        let lease = accepted.await.ok()??;
        let child = process::ProcessChild::served(lease.pid, exited);
        Some(process::SpawnedProcess {
            process_group: child.process_group(),
            child,
            stdin: None,
            output: process::ProcessOutput::Served(Served {
                lease: Arc::new(lease),
                ended,
            }),
        })
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
}

enum Note {
    Job(Job),
    Ready(u64, Probe),
    Credit(u64),
    Stopped(u64, Option<Fingerprint>),
    Stop(oneshot::Sender<()>),
}

struct Handle {
    id: u64,
    messages: mpsc::UnboundedSender<Message>,
    fingerprint: Option<Fingerprint>,
    credit: usize,
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
    probe: Option<Probe>,
    watch: Option<Watch>,
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
        let Some(probe) = self.probe.as_ref() else {
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
            Note::Job(job) => {
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
            Note::Ready(id, probe) => {
                if let Some(server) = self.servers.iter_mut().find(|server| server.id == id) {
                    server.fingerprint = Some(probe.fingerprint.clone());
                    server.credit = DEPTH;
                    if self
                        .probe
                        .as_ref()
                        .is_none_or(|known| known.files != probe.files)
                    {
                        self.watch = Watch::new(&probe.files);
                    }
                    // The fingerprint predates the watch: a change between
                    // the two shows only in a fresh fingerprint.
                    if let Some(watch) = self.watch.as_mut()
                        && fingerprint(&probe.files) != probe.fingerprint
                    {
                        watch.missed();
                    }
                    self.probe = Some(probe);
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
        });
        drop(notes);
        tokio::spawn(async move {
            let ready = run(&launch, inbox).await;
            // A server that died reported it when it did (see Owner::died).
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
    readonly: HashSet<Vec<u8>>,
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
        chunks: &mpsc::UnboundedSender<Vec<u8>>,
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
    pid: u32,
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
    chunks: mpsc::UnboundedSender<Vec<u8>>,
    activity: Arc<nanocode_process_contracts::LastActivity>,
) {
    let Some(pipe) = lease.pipes[stream].take() else {
        return;
    };
    // On the heap: in the future itself, every spawn copied it.
    let mut buffer = vec![0_u8; output::READ_BUFFER_LENGTH];
    let mut polling = false;
    loop {
        let read = if polling {
            match rustix::io::read(&pipe, &mut buffer[..]) {
                Err(rustix::io::Errno::AGAIN) => {
                    tokio::time::sleep(process::REAP_POLL_INTERVAL).await;
                    continue;
                }
                Err(rustix::io::Errno::INTR) => continue,
                read => read.map_err(io::Error::from),
            }
        } else {
            let ready = tokio::select! {
                ready = pipe.readable() => ready,
                () = ended.wait() => {
                    polling = true;
                    continue;
                }
            };
            let read = ready.and_then(|()| {
                pipe.try_io(|| match rustix::io::read(&pipe, &mut buffer[..]) {
                    Ok(0) => Err(io::ErrorKind::WouldBlock.into()),
                    read => read.map_err(io::Error::from),
                })
            });
            match read {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                read => read,
            }
        };
        match read {
            Ok(0) => {
                lease.pipes[stream].put(pipe);
                return;
            }
            Ok(length) => {
                activity.touch();
                if chunks.send(buffer[..length].to_vec()).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = chunks.send(format!("failed to read command output: {error}").into_bytes());
                return;
            }
        }
    }
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
        is_shell_identifier(name.as_bytes()) && !ready.readonly.contains(name.as_bytes())
    };
    let undiffed = |name: &OsStr| UNDIFFED.iter().any(|undiffed| name == *undiffed);
    // Commands usually have the server's environment exactly, in the same
    // order: then only secrets are exported and nothing is unset.
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
    // The server itself runs with bg_nice off and notify on (see bootstrap).
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

fn quote(out: &mut Vec<u8>, bytes: &[u8]) {
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
struct Fingerprint(Vec<Option<[i64; 7]>>);

fn fingerprint(files: &[PathBuf]) -> Fingerprint {
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

/// Changes to what fingerprint reads for a probe's files, so that a batch of
/// commands checks the files again only after one: their stats were 5 to 7%
/// of a Hand's busy samples at 512 calls at once.
/// kqueue watches each file and every directory its path and, through links,
/// its target resolve through, so a change of contents or attributes, a
/// rename, a removal or a relinked component each wakes one of them, before
/// the system call that made the change returns. Other systems have no watch
/// and check every batch.
#[cfg(target_os = "macos")]
struct Watch {
    kqueue: nix::sys::event::Kqueue,
    _watched: Vec<std::fs::File>,
    /// Report a change on the next call, whatever the kqueue says.
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
        // A missing file's nearest existing directory sees it appear.
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

fn startup_files(
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
    /// The same environment as the first command listed it.
    listed: Vec<(OsString, OsString)>,
    probe: Probe,
    weak: mpsc::WeakUnboundedSender<Message>,
    notes: mpsc::WeakUnboundedSender<Note>,
}

enum Stop {
    /// The shell did not start; a later command tries again.
    Launch(io::Error),
    /// Startup files printed output or ended the shell: commands start their
    /// own shells until those files change.
    Unready(io::Error),
    /// The server died after it was ready and already said so (Owner::died).
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

/// The server: a close-on-exec copy of its output (zsh/system's sysopen,
/// unloaded again unless the startup files loaded it) for each command's EXIT
/// trap to report its status on, so commands outlive a server that dies (zsh
/// exits when a fork fails); the startup report; then one request per line: a
/// slot forks that slot's command, w<pid> waits for one that exited without a
/// report and "c <pid>..." for ones that reported, which zsh keeps until then.
/// bg_nice is off, so commands are not niced, and notify on, so zsh drops a
/// job once it reaped it; a command's prelude restores both.
fn bootstrap(nonce: &str, dir: &Path) -> OsString {
    let mut script = format!(
        "for __nc_f in /dev/fd/<3->(N:t); do [[ -p /dev/fd/$__nc_f || -S /dev/fd/$__nc_f ]] && \\builtin exec {{__nc_f}}>&-; done\n\\builtin unset __nc_f\n\\builtin zmodload -e zsh/system && __nc_m=0 || __nc_m=1\n\\builtin zmodload zsh/system && \\builtin sysopen -w -o cloexec -u __nc_e /dev/fd/1 || \\builtin exit 1\n(( ! __nc_m )) || \\builtin zmodload -u zsh/system\n\\builtin unset __nc_m\n\\builtin print -r -- {nonce}\n\\builtin print -r -- \"${{ZDOTDIR-}}\"\n\\builtin print -r -- ${{(k)parameters[(R)*readonly*]}}\n\\builtin print -r -- $options[functionargzero] $options[bgnice] $options[notify]\n\\builtin hash -r\n\\builtin setopt monitor notify no_bg_nice\n__nc_d="
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
    let (mut child, _keeper, requests, mut events, nonce) =
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
        let _ = notes.send(Note::Ready(launch.id, probe));
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
        retiring: false,
        alive: true,
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
    owner.alive = false;
    // All the server wrote, such as the start of a command it forked just
    // before it stopped.
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

type Launched = (Child, pipe::Receiver, pipe::Sender, Events, String);

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
    let mut command = Command::new(&launch.shell);
    command
        .arg(if launch.login { "-lc" } else { "-c" })
        .arg(bootstrap(&nonce, &launch.dir))
        .current_dir(&launch.workdir)
        .env_clear()
        .envs(&launch.environment)
        .stdin(Stdio::null())
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
    Ok((child, keeper, requests, events, nonce))
}

/// The server's output: its startup report, then a line per event.
struct Events {
    pipe: pipe::Receiver,
    buffer: Vec<u8>,
}

impl Events {
    /// Reads what the pipe holds, false once every writer closed it. With
    /// force it also reads what tokio has not seen arrive yet, such as the
    /// status a command wrote before the exit kqueue just reported.
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

    /// Takes the complete lines read so far.
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
        readonly,
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
    /// Until the server names the command's process: the caller waiting for
    /// its lease, and the channel the lease returns its pipes on.
    pending: Option<(
        oneshot::Sender<Option<Lease>>,
        mpsc::UnboundedSender<Message>,
    )>,
    exit: Option<oneshot::Sender<io::Result<i32>>>,
    ended: Arc<Signal>,
    released: Option<[Option<Output>; 2]>,
    pid: Option<u32>,
    /// The status the command's EXIT trap reported.
    reported: Option<i32>,
    /// Its process exited without reporting, so the server's wait was asked.
    exited: bool,
    /// zsh is done with the process, so the slot may take another command.
    waited: bool,
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
        self.ended.fire();
        if let Some(exit) = self.exit.take() {
            let _ = exit.send(status);
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

    fn entry(&mut self, slot: usize) -> Option<&mut Entry> {
        self.entries.get_mut(slot).and_then(Option::as_mut)
    }

    fn busy(&mut self, slot: usize) -> Option<&mut Busy> {
        self.entry(slot).and_then(|entry| entry.busy.as_mut())
    }

    /// Whether a command still waits for its status.
    fn waiting(&self) -> bool {
        self.entries
            .iter()
            .flatten()
            .filter_map(|entry| entry.busy.as_ref())
            .any(|busy| busy.exit.is_some())
    }

    fn settle(&mut self, slot: usize) {
        let Some(entry) = self.entry(slot) else {
            return;
        };
        if entry
            .busy
            .as_ref()
            .is_none_or(|busy| !busy.waited || busy.released.is_none())
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
            self.entries[slot] = None;
            let _ = std::fs::remove_dir_all(self.path(slot));
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
    /// Commands that had exited when the server named them.
    gone: Vec<u32>,
    /// Commands that reported their status, for the next "c" request.
    forget: Vec<u32>,
    outbound: Vec<u8>,
    requests: pipe::Sender,
    weak: mpsc::WeakUnboundedSender<Message>,
    notes: mpsc::WeakUnboundedSender<Note>,
    live: usize,
    retiring: bool,
    alive: bool,
    /// How the server process ended, once the watcher said.
    exit: Option<String>,
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
            if self.retiring && self.live == 0 {
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
                Event::Message(Some(Message::Retire)) => self.retiring = true,
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

    /// Keeps the commands of a server that died until each reported its
    /// status or exited without one.
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
            if busy.pid.is_none() {
                // Never forked: its caller starts a shell of its own.
                if let Some((reply, _)) = busy.pending.take() {
                    let _ = reply.send(None);
                }
                entry.busy = None;
            } else if busy.exited {
                // It exited without reporting, and the server died before
                // its wait answered.
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
                // Closed once no command can report anymore; those that did
                // still get their exit.
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
        // Whatever is left can no longer report a status.
        self.fail();
    }

    /// Logs why the server stopped, with the last error zsh wrote, and tells
    /// the dispatcher at once, so later commands go elsewhere.
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
            .filter(|_| !self.retiring && self.live < MAX_JOBS);
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
        let Some(entry) = self.slots.entry(slot) else {
            return Err((reply, None));
        };
        if let Err(error) = entry.write(text, split) {
            self.slots.free.push(slot);
            return Err((reply, Some(error)));
        }
        entry.busy = Some(Busy {
            secret,
            pending: Some((reply, release)),
            exit: Some(exit),
            ended,
            released: None,
            pid: None,
            reported: None,
            exited: false,
            waited: false,
        });
        self.live += 1;
        self.outbound
            .extend_from_slice(format!("{slot}\n").as_bytes());
        Ok(())
    }

    /// Handles every line the server wrote so far, false once its output
    /// closed.
    fn read(&mut self, events: &mut Events, exits: &Exits, force: bool) -> io::Result<bool> {
        let mut open = self.handle(events, exits, force)?;
        while !self.gone.is_empty() {
            // Each wrote any status of its own before it exited, so the pipe
            // holds it now.
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
        // A command writes its status before it exits.
        let open = self.read(events, exits, true)?;
        for pid in exited.drain(..) {
            self.reap(pid);
        }
        Ok(open)
    }

    /// A command's process exited: its status is the one it reported, else
    /// the server's wait gives it (after exec, a signal, or a trap of the
    /// command's own that replaced the report).
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
                busy.waited = true;
                if busy.secret {
                    let _ = entry.erase();
                }
                self.pids.remove(&pid);
                self.forget.push(pid);
                self.live = self.live.saturating_sub(1);
                self.slots.settle(slot);
            }
            None if self.alive => {
                busy.exited = true;
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
        busy.pid = Some(pid);
        let owned = match (busy.pending.take(), std::mem::take(&mut entry.pipes)) {
            (Some((reply, release)), [Some(stdout), Some(stderr)]) => reply
                .send(Some(Lease {
                    slot,
                    pid,
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
            // Its caller stopped waiting; the dropped lease returned the pipes.
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
            busy.waited = true;
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
                if let Some((reply, _)) = busy.pending.take() {
                    let _ = reply.send(None);
                }
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

    /// Registers `changes` and returns the events the queue holds now.
    /// kevent64 with KEVENT_FLAG_IMMEDIATE returns at once when it holds none,
    /// in 0.23 µs where a zero timeout took 11.5 µs (either call, under no
    /// load). A kqueue takes only kevent64 once it had one.
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
            // A queue that had room left is empty: wait until tokio sees it
            // readable again.
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
