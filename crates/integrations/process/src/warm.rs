use std::{
    ffi::{OsStr, OsString},
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};

use nanocode_sync::{Atom, Inbox, Slot};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{SessionEnvironment, process, selection::Shell};

const SCRIPT_BYTES: usize = 4096;

const LOADER: &str = "for __nc_f in /dev/fd/*; do __nc_f=${__nc_f##*/}; case $__nc_f in 0|1|2) ;; *) builtin eval \"exec $__nc_f>&-\";; esac; done; builtin unset __nc_f; builtin printf '\\0'; IFS= builtin read -r -d '' BASH_EXECUTION_STRING || builtin exit; builtin eval \"$BASH_EXECUTION_STRING\"; IFS= builtin read -r -d '' BASH_EXECUTION_STRING || builtin exit; exec 0</dev/null; SECONDS=0; builtin : \"$BASH\"; builtin eval \"$BASH_EXECUTION_STRING\"";

static POOL: OnceLock<Pool> = OnceLock::new();

pub(super) async fn spawn(
    script: &str,
    workdir: &Path,
    shell: &Shell,
    login: bool,
    environment: &[(OsString, OsString)],
) -> Option<process::SpawnedProcess> {
    if shell.name() != "bash" || script.len() > SCRIPT_BYTES || script.contains('\0') {
        return None;
    }
    let (moved, base): (Vec<_>, Vec<_>) = environment
        .iter()
        .cloned()
        .partition(|(name, _)| SessionEnvironment::controls(name));
    let mut input = Vec::with_capacity(script.len() + 128);
    for (name, value) in &moved {
        input.extend_from_slice(b"builtin export ");
        input.extend_from_slice(name.as_bytes());
        input.push(b'=');
        quote(&mut input, value.as_bytes());
        input.push(b';');
    }
    input.push(0);
    input.extend_from_slice(script.as_bytes());
    input.push(0);
    let pool = POOL.get_or_init(Pool::new);
    let (lane, evicted) = pool.lane(shell, login, workdir, &base);
    let (taken, discarded) = lane.take(pool);
    let fill = lane.count.load(Ordering::Relaxed) < lane.depth.load(Ordering::Relaxed)
        && lane
            .filling
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
    if fill || !evicted.is_empty() || !discarded.is_empty() {
        let refill = Arc::clone(&lane);
        drop(tokio::task::spawn_blocking(move || {
            evicted.into_iter().chain(discarded).for_each(Idle::kill);
            if fill {
                pool.fill(&refill);
                refill.filling.store(false, Ordering::Release);
            }
        }));
    }
    taken?.start(&input).await.ok()
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

fn identity(directory: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(directory)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

fn startup_files(login: bool, environment: &[(OsString, OsString)]) -> Vec<PathBuf> {
    let variable = |name: &str| {
        environment
            .iter()
            .find(|(key, _)| key == OsStr::new(name))
            .map(|(_, value)| PathBuf::from(value))
    };
    let mut files: Vec<PathBuf> = variable("BASH_ENV").into_iter().collect();
    if login {
        files.extend(
            [
                "/etc/profile",
                "/etc/profile.d",
                "/etc/bash.bashrc",
                "/etc/bashrc",
            ]
            .map(PathBuf::from),
        );
        if let Some(home) = variable("HOME") {
            files.extend(
                [".bash_profile", ".bash_login", ".profile", ".bashrc"].map(|file| home.join(file)),
            );
        }
        if cfg!(target_os = "macos") {
            files.extend(["/etc/paths", "/etc/paths.d"].map(PathBuf::from));
        }
    }
    files
}

struct Idle {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    ready: bool,
    workdir: (u64, u64),
    files: Fingerprint,
}

impl Idle {
    fn alive(&mut self) -> bool {
        if self.ready {
            return matches!(self.child.try_wait(), Ok(None));
        }
        let mut byte = [0_u8; 1];
        match rustix::io::read(&self.stdout, &mut byte) {
            Ok(1) => {
                self.ready = byte[0] == 0;
                self.ready
            }
            Err(rustix::io::Errno::AGAIN) => true,
            _ => false,
        }
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    async fn start(self, input: &[u8]) -> io::Result<process::SpawnedProcess> {
        let Self {
            mut child,
            stdin,
            stdout,
            stderr,
            ready,
            ..
        } = self;
        let pid = child.id();
        let mut stdin = tokio::process::ChildStdin::from_std(stdin)?;
        let mut stdout = tokio::process::ChildStdout::from_std(stdout)?;
        let stderr = tokio::process::ChildStderr::from_std(stderr)?;
        let waited = process::ProcessChild::new(
            move || {
                child
                    .try_wait()
                    .map(|status| status.map(process::exit_code))
            },
            pid,
            true,
        );
        if !ready {
            let mut marker = [0_u8; 1];
            stdout.read_exact(&mut marker).await?;
            if marker[0] != 0 {
                return Err(io::Error::other(
                    "the warm shell printed before its command",
                ));
            }
        }
        stdin.write_all(input).await?;
        drop(stdin);
        Ok(process::SpawnedProcess {
            process_group: waited.process_group(),
            child: waited,
            stdin: None,
            output: process::ProcessOutput::Pipes {
                stdout: Some(stdout),
                stderr: Some(stderr),
            },
        })
    }
}

struct Lane {
    shell: PathBuf,
    args: Vec<String>,
    login: bool,
    workdir: PathBuf,
    environment: Vec<(OsString, OsString)>,
    files: Vec<PathBuf>,
    idle: Inbox<Arc<Slot<Idle>>>,
    count: AtomicUsize,
    depth: AtomicUsize,
    used: AtomicU64,
    filling: AtomicBool,
}

impl Lane {
    fn matches(
        &self,
        shell: &Shell,
        login: bool,
        workdir: &Path,
        environment: &[(OsString, OsString)],
    ) -> bool {
        self.login == login
            && self.shell == shell.path()
            && self.workdir == workdir
            && self.environment == environment
    }

    fn take(&self, pool: &Pool) -> (Option<Idle>, Vec<Idle>) {
        self.used.store(pool.now(), Ordering::Relaxed);
        let files = fingerprint(&self.files);
        let workdir = identity(&self.workdir);
        let mut alive = Vec::new();
        let mut discarded = Vec::new();
        for entry in self.idle.drain() {
            let Some(mut idle) = entry.take() else {
                continue;
            };
            if idle.files == files && Some(idle.workdir) == workdir && idle.alive() {
                alive.push(idle);
            } else {
                pool.release(self);
                discarded.push(idle);
            }
        }
        let limit = pool.limit;
        let grow = |depth: usize| Some((depth + 1).min(limit));
        if alive.is_empty() {
            self.depth.fetch_max(1, Ordering::Relaxed);
            return (None, discarded);
        }
        let chosen = alive.remove(alive.iter().position(|idle| idle.ready).unwrap_or(0));
        pool.release(self);
        if !chosen.ready {
            let _ = self
                .depth
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, grow);
        } else if alive.iter().any(|idle| idle.ready) {
            let _ = self
                .depth
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |depth| {
                    Some(depth.saturating_sub(1).max(1))
                });
        }
        for idle in alive {
            self.idle.push(Arc::new(Slot::new(Some(idle))));
        }
        (Some(chosen), discarded)
    }

    fn launch(&self) -> io::Result<Idle> {
        let workdir = identity(&self.workdir).ok_or_else(|| io::Error::other("workdir is gone"))?;
        let files = fingerprint(&self.files);
        let mut child = Command::new(&self.shell)
            .args(&self.args)
            .current_dir(&self.workdir)
            .env_clear()
            .envs(self.environment.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("a warm shell has no pipes"));
        };
        let idle = Idle {
            child,
            stdin,
            stdout,
            stderr,
            ready: false,
            workdir,
            files,
        };
        if let Err(error) = rustix::io::ioctl_fionbio(&idle.stdout, true) {
            idle.kill();
            return Err(error.into());
        }
        Ok(idle)
    }

    fn drain(&self, pool: &Pool) -> Vec<Idle> {
        self.idle
            .drain()
            .into_iter()
            .filter_map(|entry| entry.take())
            .inspect(|_| pool.release(self))
            .collect()
    }
}

struct Pool {
    lanes: Atom<Vec<Arc<Lane>>>,
    total: AtomicUsize,
    limit: usize,
    epoch: Instant,
}

impl Pool {
    fn new() -> Self {
        Self {
            lanes: Atom::new(Vec::new()),
            total: AtomicUsize::new(0),
            limit: std::thread::available_parallelism().map_or(1, usize::from),
            epoch: Instant::now(),
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn release(&self, lane: &Lane) {
        lane.count.fetch_sub(1, Ordering::Relaxed);
        self.total.fetch_sub(1, Ordering::Relaxed);
    }

    fn lane(
        &self,
        shell: &Shell,
        login: bool,
        workdir: &Path,
        environment: &[(OsString, OsString)],
    ) -> (Arc<Lane>, Vec<Idle>) {
        let find = |lanes: &Vec<Arc<Lane>>| {
            lanes
                .iter()
                .find(|lane| lane.matches(shell, login, workdir, environment))
                .cloned()
        };
        if let Some(lane) = self.lanes.read(find) {
            return (lane, Vec::new());
        }
        let fresh = Arc::new(Lane {
            shell: shell.path().to_owned(),
            args: shell.args(LOADER, login),
            login,
            workdir: workdir.to_owned(),
            environment: environment.to_vec(),
            files: startup_files(login, environment),
            idle: Inbox::default(),
            count: AtomicUsize::new(0),
            depth: AtomicUsize::new(0),
            used: AtomicU64::new(self.now()),
            filling: AtomicBool::new(false),
        });
        let (lane, removed) = self.lanes.update(|lanes| {
            if let Some(lane) = find(lanes) {
                return (lane, None);
            }
            let removed = (lanes.len() >= self.limit)
                .then(|| {
                    lanes
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, lane)| lane.used.load(Ordering::Relaxed))
                        .map(|(index, _)| index)
                })
                .flatten()
                .map(|index| lanes.remove(index));
            lanes.push(Arc::clone(&fresh));
            (Arc::clone(&fresh), removed)
        });
        let evicted = removed.map_or_else(Vec::new, |removed| removed.drain(self));
        (lane, evicted)
    }

    fn fill(&self, lane: &Arc<Lane>) {
        loop {
            let depth = lane.depth.load(Ordering::Relaxed);
            if lane
                .count
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    (count < depth).then_some(count + 1)
                })
                .is_err()
            {
                return;
            }
            if self
                .total
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                    (total < self.limit).then_some(total + 1)
                })
                .is_err()
                && !self.evict(lane)
            {
                lane.count.fetch_sub(1, Ordering::Relaxed);
                return;
            }
            match lane.launch() {
                Ok(idle) => lane.idle.push(Arc::new(Slot::new(Some(idle)))),
                Err(_) => {
                    self.release(lane);
                    return;
                }
            }
            if !self
                .lanes
                .read(|lanes| lanes.iter().any(|listed| Arc::ptr_eq(listed, lane)))
            {
                lane.drain(self).into_iter().for_each(Idle::kill);
                return;
            }
        }
    }

    fn evict(&self, keep: &Arc<Lane>) -> bool {
        let used = keep.used.load(Ordering::Relaxed);
        let victim = self.lanes.read(|lanes| {
            lanes
                .iter()
                .filter(|lane| {
                    !Arc::ptr_eq(lane, keep)
                        && lane.count.load(Ordering::Relaxed) > 0
                        && lane.used.load(Ordering::Relaxed) < used
                })
                .min_by_key(|lane| lane.used.load(Ordering::Relaxed))
                .cloned()
        });
        let Some(victim) = victim else {
            return false;
        };
        let mut entries = victim.idle.drain().into_iter();
        let Some(idle) = entries.find_map(|entry| entry.take()) else {
            return false;
        };
        for entry in entries {
            victim.idle.push(entry);
        }
        victim.count.fetch_sub(1, Ordering::Relaxed);
        idle.kill();
        true
    }
}
