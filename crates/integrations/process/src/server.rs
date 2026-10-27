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
    io::{AsyncBufReadExt, BufReader},
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
        let (pid, started) = oneshot::channel();
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
            pid,
            exit,
            ended: Arc::clone(&ended),
            reply,
        };
        pool.dispatcher(self.root()?, request.shell, request.login)
            .send(Note::Job(job))
            .ok()?;
        let lease = accepted.await.ok()??;
        let child = process::ProcessChild::served(started, exited);
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
    pid: oneshot::Sender<u32>,
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
        if fingerprint(&probe.files) == probe.fingerprint {
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
    readonly: HashSet<Vec<u8>>,
    argzero: bool,
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
    let mut buffer = [0_u8; output::READ_BUFFER_LENGTH];
    let mut polling = false;
    loop {
        let read = if polling {
            match rustix::io::read(&pipe, &mut buffer) {
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
                pipe.try_io(|| match rustix::io::read(&pipe, &mut buffer) {
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
    for (name, value) in &job.environment {
        let secret = job.secrets.contains(name);
        if undiffed(name) || (!secret && ready.environment.get(name) == Some(value)) {
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
    for name in ready.environment.keys() {
        if undiffed(name) || job.environment.iter().any(|(present, _)| present == name) {
            continue;
        }
        if !settable(name) {
            return None;
        }
        prefix.extend_from_slice(b"\\builtin unset ");
        prefix.extend_from_slice(name.as_bytes());
        prefix.extend_from_slice(b"; ");
    }
    let mut random = [0_u8; 2];
    getrandom::fill(&mut random).ok()?;
    prefix.extend_from_slice(
        format!(
            "RANDOM={}; SECONDS=0; {}",
            u16::from_ne_bytes(random),
            if ready.argzero {
                "\\builtin setopt function_argzero; "
            } else {
                ""
            }
        )
        .as_bytes(),
    );
    let split = prefix.len();
    prefix.extend_from_slice(job.script.as_bytes());
    Some((prefix, split))
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
    probe: Probe,
    weak: mpsc::WeakUnboundedSender<Message>,
    notes: mpsc::WeakUnboundedSender<Note>,
}

enum Stop {
    Unready(io::Error),
    Failed(io::Error),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unready(error) => write!(formatter, "before it was ready: {error}"),
            Self::Failed(error) => error.fmt(formatter),
        }
    }
}

fn bootstrap(nonce: &str, dir: &Path) -> OsString {
    let mut script = format!(
        "\\builtin print -r -- {nonce}\n\\builtin print -r -- \"${{ZDOTDIR-}}\"\n\\builtin print -r -- ${{(k)parameters[(R)*readonly*]}}\n\\builtin print -r -- $options[functionargzero]\n\\builtin hash -r\n\\builtin setopt monitor\n__nc_d="
    )
    .into_bytes();
    quote(&mut script, dir.as_os_str().as_bytes());
    script.extend_from_slice(
        b"\n\\exec 3>&1 0<\"$__nc_d/q\" 1>/dev/null 2>/dev/null 9</dev/null || \\builtin exit 1\n\
while \\builtin read -r __nc_l; do\n\
if [[ $__nc_l = w* ]]; then\n\
__nc_s=0; \\builtin wait \"${__nc_l#w}\" || __nc_s=$?\n\
\\builtin print -ru3 -- \"x ${__nc_l#w} $__nc_s\"\n\
else\n\
( \\builtin cd -q -- \"$__nc_d/$__nc_l\" && \\exec 1>|1 2>|2 0<&9 9<&- 3>&- && \\builtin unset __nc_l __nc_d __nc_s && \\builtin unsetopt function_argzero && \\builtin source zsh ) &\n\
\\builtin print -ru3 -- \"s $__nc_l $!\"\n\
fi\n\
done\n",
    );
    OsString::from_vec(script)
}

async fn run(launch: &Launch, inbox: mpsc::UnboundedReceiver<Message>) -> Result<(), Stop> {
    private_dir(&launch.dir).map_err(Stop::Unready)?;
    let result = serve(launch, inbox).await;
    let _ = std::fs::remove_dir_all(&launch.dir);
    result
}

async fn serve(launch: &Launch, inbox: mpsc::UnboundedReceiver<Message>) -> Result<(), Stop> {
    let (mut child, _keeper, requests, mut events, nonce) =
        launch_shell(launch).map_err(Stop::Unready)?;
    let ready = read_ready(&mut events, &nonce, launch).await;
    let (ready, probe) = match ready {
        Ok(ready) => ready,
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(Stop::Unready(error));
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
        outbound: Vec::new(),
        requests,
        weak: launch.weak.clone(),
        notes: launch.notes.clone(),
        live: 0,
        retiring: false,
    };
    let exits = Exits::new().map_err(Stop::Failed)?;
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
    let stopped = owner.serve(&mut watcher, events, inbox, exits).await;
    owner.fail();
    drop(kill);
    let _ = watcher.await;
    match stopped {
        Ok(Some(reply)) => {
            let _ = reply.send(());
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(error) => Err(Stop::Failed(error)),
    }
}

type Events = BufReader<pipe::Receiver>;

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
    let events = pipe::Receiver::from_owned_fd(reader.into())?;
    Ok((child, keeper, requests, BufReader::new(events), nonce))
}

async fn line(events: &mut Events) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    events.read_until(b'\n', &mut line).await?;
    if line.pop() != Some(b'\n') {
        return Err(io::Error::other("the shell stopped while it started"));
    }
    Ok(line)
}

async fn read_ready(
    events: &mut Events,
    nonce: &str,
    launch: &Launch,
) -> io::Result<(Ready, Probe)> {
    if line(events).await? != nonce.as_bytes() {
        return Err(io::Error::other(
            "the shell printed output while it started",
        ));
    }
    let zdotdir = OsString::from_vec(line(events).await?);
    let readonly = line(events)
        .await?
        .split(u8::is_ascii_whitespace)
        .filter(|name| !name.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    let argzero = line(events).await? == b"on";
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
        readonly,
        argzero,
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
    started: Option<oneshot::Sender<u32>>,
    exit: Option<oneshot::Sender<io::Result<i32>>>,
    ended: Arc<Signal>,
    released: Option<[Option<Output>; 2]>,
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
        self.started = None;
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

    fn settle(&mut self, slot: usize) {
        let Some(entry) = self.entry(slot) else {
            return;
        };
        if entry
            .busy
            .as_ref()
            .is_none_or(|busy| busy.exit.is_some() || busy.released.is_none())
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
    Line(io::Result<usize>),
    Exited(io::Result<()>),
    Child(io::Result<std::process::ExitStatus>),
    Writable(io::Result<()>),
}

struct Owner {
    id: u64,
    ready: Ready,
    slots: Slots,
    pids: HashMap<u32, usize>,
    outbound: Vec<u8>,
    requests: pipe::Sender,
    weak: mpsc::WeakUnboundedSender<Message>,
    notes: mpsc::WeakUnboundedSender<Note>,
    live: usize,
    retiring: bool,
}

impl Owner {
    async fn serve(
        &mut self,
        watcher: &mut oneshot::Receiver<io::Result<std::process::ExitStatus>>,
        mut events: Events,
        mut inbox: mpsc::UnboundedReceiver<Message>,
        mut exits: Exits,
    ) -> io::Result<Option<oneshot::Sender<()>>> {
        let mut exited = Vec::new();
        let mut line = Vec::new();
        loop {
            if self.retiring && self.live == 0 {
                return Ok(None);
            }
            let event = tokio::select! {
                message = inbox.recv() => Event::Message(message),
                read = events.read_until(b'\n', &mut line) => Event::Line(read),
                ready = exits.next(&mut exited) => Event::Exited(ready),
                status = &mut *watcher => Event::Child(status.unwrap_or_else(|_| Err(io::Error::other("the shell server's watcher stopped")))),
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
                Event::Line(read) => {
                    if read? == 0 {
                        return Err(io::Error::other("the shell server closed its output"));
                    }
                    if line.last() == Some(&b'\n') {
                        self.event(&String::from_utf8_lossy(&line), &exits);
                        line.clear();
                    }
                }
                Event::Exited(ready) => {
                    ready?;
                    for pid in exited.drain(..) {
                        self.outbound
                            .extend_from_slice(format!("w{pid}\n").as_bytes());
                    }
                }
                Event::Child(status) => {
                    return Err(io::Error::other(format!(
                        "the shell server exited: {}",
                        status?
                    )));
                }
                Event::Writable(writable) => writable?,
            }
            self.flush()?;
        }
    }

    fn flush(&mut self) -> io::Result<()> {
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
        match started {
            Ok(true) => {}
            Ok(false) => self.credit(),
            Err((reply, error)) => {
                if let Some(error) = error {
                    tracing::debug!(%error, "the shell server could not prepare a command");
                }
                let _ = reply.send(None);
                self.credit();
            }
        }
    }

    fn prepare(
        &mut self,
        job: Job,
        text: &[u8],
        split: usize,
        release: mpsc::UnboundedSender<Message>,
    ) -> Result<bool, (oneshot::Sender<Option<Lease>>, Option<io::Error>)> {
        let secret = !job.secrets.is_empty();
        let Job {
            pid,
            exit,
            ended,
            reply,
            ..
        } = job;
        let slot = match self.slots.lease() {
            Ok(slot) => slot,
            Err(error) => return Err((reply, Some(error))),
        };
        let Some(entry) = self.slots.entry(slot) else {
            return Err((reply, None));
        };
        let written = entry.write(text, split);
        if let Err(error) = written {
            self.slots.free.push(slot);
            return Err((reply, Some(error)));
        }
        let [Some(stdout), Some(stderr)] = std::mem::take(&mut entry.pipes) else {
            return Err((reply, None));
        };
        entry.busy = Some(Busy {
            secret,
            started: Some(pid),
            exit: Some(exit),
            ended,
            released: None,
        });
        let lease = Lease {
            slot,
            pipes: [Slot::new(Some(stdout)), Slot::new(Some(stderr))],
            release,
        };
        if reply.send(Some(lease)).is_err() {
            if let Some(busy) = self.slots.busy(slot) {
                busy.end(Err(io::Error::other("the command was cancelled")));
            }
            return Ok(false);
        }
        self.live += 1;
        self.outbound
            .extend_from_slice(format!("{slot}\n").as_bytes());
        Ok(true)
    }

    fn event(&mut self, line: &str, exits: &Exits) {
        let mut fields = line.split_ascii_whitespace();
        let (Some(kind), Some(Ok(first))) = (fields.next(), fields.next().map(str::parse::<u32>))
        else {
            return;
        };
        let second = fields.next().map(str::parse::<i32>);
        let Ok(slot) = usize::try_from(first) else {
            return;
        };
        match (kind, second) {
            ("s", Some(Ok(pid))) => {
                self.credit();
                let (Ok(pid), Some(busy)) = (u32::try_from(pid), self.slots.busy(slot)) else {
                    return;
                };
                if let Some(started) = busy.started.take() {
                    let _ = started.send(pid);
                }
                self.pids.insert(pid, slot);
                if exits.watch(pid) {
                    self.outbound
                        .extend_from_slice(format!("w{pid}\n").as_bytes());
                }
            }
            ("x", Some(Ok(status))) => {
                let Some(slot) = self.pids.remove(&first) else {
                    return;
                };
                if let Some(entry) = self.slots.entry(slot)
                    && let Some(busy) = entry.busy.as_mut()
                {
                    busy.end(Ok(status));
                    if busy.secret {
                        let _ = entry.erase();
                    }
                    self.live = self.live.saturating_sub(1);
                }
                self.slots.settle(slot);
            }
            _ => {}
        }
    }

    fn fail(&mut self) {
        for entry in self.slots.entries.iter_mut().flatten() {
            if let Some(busy) = entry.busy.as_mut() {
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
    const NOW: nix::libc::timespec = nix::libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };

    fn new() -> io::Result<Self> {
        use std::os::fd::AsFd;
        let kqueue = nix::sys::event::Kqueue::new()?;
        let ready = pipe::Receiver::from_owned_fd_unchecked(kqueue.as_fd().try_clone_to_owned()?)?;
        Ok(Self { kqueue, ready })
    }

    fn event(pid: usize, flags: nix::sys::event::EvFlags) -> nix::sys::event::KEvent {
        use nix::sys::event::{EventFilter, FilterFlag, KEvent};
        KEvent::new(
            pid,
            EventFilter::EVFILT_PROC,
            flags,
            FilterFlag::NOTE_EXIT,
            0,
            0,
        )
    }

    fn watch(&self, pid: u32) -> bool {
        use nix::sys::event::EvFlags;
        let event = Self::event(pid as usize, EvFlags::EV_ADD | EvFlags::EV_ONESHOT);
        self.kqueue
            .kevent(&[event], &mut [], Some(Self::NOW))
            .is_err()
    }

    async fn next(&mut self, exited: &mut Vec<u32>) -> io::Result<()> {
        use nix::sys::event::{EvFlags, FilterFlag};
        self.ready.readable().await?;
        let mut events = [Self::event(0, EvFlags::empty()); 64];
        let count =
            self.ready.try_io(
                || match self.kqueue.kevent(&[], &mut events, Some(Self::NOW))? {
                    0 => Err(io::ErrorKind::WouldBlock.into()),
                    count => Ok(count),
                },
            );
        let count = match count {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => 0,
            count => count?,
        };
        exited.extend(
            events[..count]
                .iter()
                .filter(|event| event.fflags().contains(FilterFlag::NOTE_EXIT))
                .filter_map(|event| u32::try_from(event.ident()).ok()),
        );
        Ok(())
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
