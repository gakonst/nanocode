use crate::{
    bubblewrap::Bubblewrap,
    budget::{Admitted, Budget, Busy, Stopped},
    confine,
    egress::{Allowlist, Egress, Limits, Secret, Tls, tailnet_host},
};
use futures_util::future::BoxFuture;
use nanocode_sync::Signal;
use std::{
    collections::VecDeque,
    io,
    iter::once,
    net::SocketAddr,
    os::fd::{OwnedFd, RawFd},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    net::{UnixStream, unix::pipe},
    process::{Child, Command},
    task::JoinHandle,
    time::{error::Elapsed, timeout},
};

const PIPE: usize = 64 * 1024;
const SOCKET_NAME: usize = 16;
const PROXY_SOCKET: &str = "proxy.sock";

pub enum Confinement {
    Bubblewrap {
        bwrap: PathBuf,
        launcher: PathBuf,
        root: PathBuf,
        work: PathBuf,
    },
    Landlock {
        launcher: PathBuf,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub output: usize,
    pub errors: usize,
    pub header: usize,
    pub body: usize,
}

pub struct Reach {
    pub tls: Arc<Tls>,
    pub tailnet: Option<SocketAddr>,
    pub limits: Limits,
}

pub struct Runner {
    confinement: Confinement,
    scratch: PathBuf,
    sockets: PathBuf,
    reach: Reach,
    caps: Caps,
    budget: Budget,
}

impl Drop for Runner {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.sockets).ok();
    }
}

#[derive(Clone, Copy)]
pub struct Read<'a> {
    pub path: &'a Path,
    pub name: &'a str,
}

pub struct Call<'a> {
    pub program: Read<'a>,
    pub reads: &'a [Read<'a>],
    pub input: Option<&'a str>,
    pub within: Duration,
    pub admitted: Option<&'a Admitted<'a>>,
    pub hosts: &'a [String],
    pub tailnet: bool,
    pub refusal: fn(&str) -> String,
    pub secrets: Vec<Secret>,
    pub host: Option<Arc<dyn Host>>,
}

pub enum Seen<'a> {
    Mounted {
        root: &'a Path,
        work: &'a Path,
        proxy: PathBuf,
        host: Option<RawFd>,
    },
    InPlace {
        work: &'a Path,
        port: u16,
        host: Option<RawFd>,
    },
}

impl Seen<'_> {
    #[must_use]
    pub fn read(&self, read: Read<'_>) -> PathBuf {
        match self {
            Self::Mounted { root, .. } => root.join(read.name),
            Self::InPlace { .. } => read.path.into(),
        }
    }
}

pub enum Ran {
    Exited {
        output: Vec<u8>,
        errors: Vec<u8>,
        denied: Vec<String>,
        unreachable: Vec<String>,
    },
    TimedOut,
    Overflowed,
    Stopped(Stopped),
}

pub struct Reply {
    pub header: Vec<u8>,
    pub parts: Vec<Arc<[u8]>>,
}

pub trait Host: Send + Sync {
    fn answer(&self, header: Vec<u8>, body: Vec<u8>) -> BoxFuture<'_, io::Result<Reply>>;

    fn busy(&self, busy: Busy) -> Vec<u8>;

    fn failed(&self, failure: io::Error);
}

impl Runner {
    pub fn new(
        confinement: Confinement,
        scratch: PathBuf,
        sockets: PathBuf,
        reach: Reach,
        caps: Caps,
        budget: Budget,
    ) -> io::Result<Self> {
        std::fs::create_dir(&sockets)?;
        Ok(Self {
            confinement,
            scratch,
            sockets,
            reach,
            caps,
            budget,
        })
    }

    #[must_use]
    pub const fn budget(&self) -> &Budget {
        &self.budget
    }

    pub async fn run(
        &self,
        call: Call<'_>,
        compose: impl FnOnce(&Seen<'_>, &mut Command),
    ) -> io::Result<Ran> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut cleanup = Cleanup {
            work: self.scratch.join(&id),
            group: None,
        };
        std::fs::create_dir_all(&cleanup.work)?;
        let socket = self.sockets.join(format!("{}.sock", &id[..SOCKET_NAME]));
        let channel = match call.host {
            Some(host) => {
                let confine::Channel {
                    host: stream,
                    program,
                    input,
                } = confine::channel()?;
                stream.set_nonblocking(true)?;
                let input = pipe::Sender::from_owned_fd(OwnedFd::from(input))?;
                Some((UnixStream::from_std(stream)?, host, program, input))
            }
            None => None,
        };
        let host = channel.as_ref().map(|_| confine::CHANNEL_DESCRIPTOR);
        let allowlist = Allowlist {
            hosts: call
                .hosts
                .iter()
                .filter(|host| call.tailnet || !tailnet_host(host))
                .cloned()
                .collect(),
            private_hosts: Vec::new(),
            secrets: call.secrets,
            tls: Arc::clone(&self.reach.tls),
            tailnet: self.reach.tailnet.filter(|_| call.tailnet),
            refusal: call.refusal,
            limits: self.reach.limits,
        };
        let egress = match self.confinement {
            Confinement::Bubblewrap { .. } => Egress::unix(&socket, allowlist)?,
            Confinement::Landlock { .. } => Egress::tcp(allowlist).await?,
        };
        let reads = || once(call.program).chain(call.reads.iter().copied());
        let mut command = match &self.confinement {
            Confinement::Bubblewrap {
                bwrap,
                launcher,
                root,
                work,
            } => {
                let mut sandbox = Bubblewrap::new(bwrap, &cleanup.work, work);
                for Read { path, name } in reads() {
                    sandbox.read(path, &root.join(name));
                }
                let proxy = root.join(PROXY_SOCKET);
                sandbox.bind(&socket, &proxy);
                let program = root.join(call.program.name);
                let mut child = match host {
                    Some(_) => sandbox.command_with_channel(launcher, program),
                    None => sandbox.command(program),
                };
                let seen = Seen::Mounted {
                    root,
                    work,
                    proxy,
                    host,
                };
                compose(&seen, &mut child);
                child
            }
            Confinement::Landlock { launcher } => {
                let port = egress.port();
                let policy = confine::Policy {
                    read: reads().map(|read| read.path.into()).collect(),
                    write: cleanup.work.clone(),
                    port,
                };
                let mut child = match host {
                    Some(_) => policy.command_with_channel(launcher, call.program.path)?,
                    None => policy.command(launcher, call.program.path)?,
                };
                let seen = Seen::InPlace {
                    work: &cleanup.work,
                    port,
                    host,
                };
                compose(&seen, &mut child);
                child
            }
        };
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let (channel, input) = match channel {
            Some((stream, host, program, input)) => {
                command.stdin(Stdio::from(program));
                (Some((stream, host)), Some(input))
            }
            None => {
                command.stdin(Stdio::piped());
                (None, None)
            }
        };
        let mut child = command.spawn()?;
        drop(command);
        if let Confinement::Landlock { .. } = self.confinement {
            cleanup.group = child
                .id()
                .and_then(|id| i32::try_from(id).ok())
                .and_then(rustix::process::Pid::from_raw);
        }
        let stdin: Box<dyn AsyncWrite + Send + Unpin> = match input {
            Some(input) => Box::new(input),
            None => Box::new(child.stdin.take().ok_or_else(|| missing("no stdin"))?),
        };
        let overflow = Arc::new(Signal::new());
        let output = read_all(child.stdout.take(), self.caps.output, Arc::clone(&overflow));
        let errors = read_tail(child.stderr.take(), self.caps.errors);
        let watch = self.budget.watch(child.id(), call.admitted);
        let exchanged = exchange(&mut child, stdin, call.input, call.within, &overflow, watch);
        let ran = match channel {
            None => exchanged.await,
            Some((stream, host)) => {
                let base = call.admitted.map_or(0, Admitted::expected);
                let mut held = Vec::new();
                let charge = |bytes| self.budget.charge(&mut held, base, bytes);
                let serving = serve(stream, host.as_ref(), self.caps, charge);
                tokio::pin!(exchanged, serving);
                let mut served = false;
                loop {
                    tokio::select! {
                        ran = &mut exchanged => break ran,
                        failure = &mut serving, if !served => {
                            served = true;
                            if let Err(failure) = failure {
                                host.failed(failure);
                            }
                        }
                    }
                }
            }
        };
        cleanup.kill();
        let (wrote, exited) = match ran {
            Ok(ran) => ran,
            Err(stopped) => return Ok(Ran::Stopped(stopped)),
        };
        wrote?;
        let Ok(exited) = exited else {
            return Ok(Ran::TimedOut);
        };
        let Some(exited) = exited else {
            child.start_kill().ok();
            return Ok(Ran::Overflowed);
        };
        exited?;
        let (output, errors) = (output.await??, errors.await??);
        drop(cleanup);
        let Some(output) = output else {
            return Ok(Ran::Overflowed);
        };
        Ok(Ran::Exited {
            output,
            errors,
            denied: egress.denied(),
            unreachable: egress.unreachable(),
        })
    }
}

fn missing(what: &'static str) -> io::Error {
    io::Error::other(what)
}

struct Cleanup {
    work: PathBuf,
    group: Option<rustix::process::Pid>,
}

impl Cleanup {
    fn kill(&mut self) {
        if let Some(group) = self.group.take() {
            rustix::process::kill_process_group(group, rustix::process::Signal::KILL).ok();
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.kill();
        std::fs::remove_dir_all(&self.work).ok();
    }
}

async fn serve(
    mut stream: UnixStream,
    host: &dyn Host,
    caps: Caps,
    mut charge: impl FnMut(usize) -> Result<(), Busy>,
) -> io::Result<()> {
    while let Some(header) = frame(&mut stream, caps.header).await? {
        let body = frame(&mut stream, caps.body)
            .await?
            .ok_or_else(|| missing("a frame without its body"))?;
        let Reply { header, parts } = host.answer(header, body).await?;
        let length: usize = parts.iter().map(|part| part.len()).sum();
        let (header, parts, length) = match length {
            0 => (header, parts, 0),
            length => match charge(length) {
                Ok(()) => (header, parts, length),
                Err(busy) => (host.busy(busy), Vec::new(), 0),
            },
        };
        stream.write_all(&prefix(header.len())?).await?;
        stream.write_all(&header).await?;
        stream.write_all(&prefix(length)?).await?;
        for part in &parts {
            stream.write_all(part).await?;
        }
    }
    Ok(())
}

fn prefix(length: usize) -> io::Result<[u8; 4]> {
    Ok(u32::try_from(length)
        .map_err(io::Error::other)?
        .to_be_bytes())
}

async fn frame(stream: &mut UnixStream, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    match stream.read_exact(&mut length).await {
        Ok(_) => {}
        Err(failure) if failure.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(failure) => return Err(failure),
    }
    let length = usize::try_from(u32::from_be_bytes(length)).map_err(io::Error::other)?;
    if length > limit {
        return Err(io::Error::other(format!("a host frame of {length} bytes")));
    }
    let mut part = vec![0; length];
    stream.read_exact(&mut part).await?;
    Ok(Some(part))
}

type Exchanged = (
    io::Result<()>,
    Result<Option<io::Result<ExitStatus>>, Elapsed>,
);

async fn exchange(
    child: &mut Child,
    mut stdin: impl AsyncWrite + Unpin,
    input: Option<&str>,
    within: Duration,
    overflow: &Signal,
    watch: impl Future<Output = Stopped>,
) -> Result<Exchanged, Stopped> {
    let ran = async {
        let wrote = match input {
            Some(input) => stdin.write_all(input.as_bytes()).await,
            None => Ok(()),
        };
        drop(stdin);
        let exited = timeout(within, async {
            tokio::select! {
                status = child.wait() => Some(status),
                () = overflow.wait() => None,
            }
        })
        .await;
        (wrote, exited)
    };
    tokio::select! {
        ran = ran => Ok(ran),
        stop = watch => Err(stop),
    }
}

type Reading<T> = JoinHandle<io::Result<T>>;

fn read_tail<R>(pipe: Option<R>, most: usize) -> Reading<Vec<u8>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut pipe = pipe.ok_or_else(|| missing("no pipe"))?;
        let mut tail = VecDeque::new();
        let mut chunk = vec![0; PIPE];
        loop {
            let read = pipe.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            tail.extend(&chunk[..read]);
            let excess = tail.len().saturating_sub(most);
            tail.drain(..excess);
        }
        Ok(tail.into())
    })
}

fn read_all<R>(pipe: Option<R>, most: usize, overflow: Arc<Signal>) -> Reading<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let pipe = pipe.ok_or_else(|| missing("no pipe"))?;
        let limit = u64::try_from(most)
            .map_err(io::Error::other)?
            .saturating_add(1);
        pipe.take(limit).read_to_end(&mut bytes).await?;
        if bytes.len() > most {
            overflow.fire();
            return Ok(None);
        }
        Ok(Some(bytes))
    })
}
