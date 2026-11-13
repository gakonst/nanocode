use std::{
    ffi::OsString,
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

static POOL: OnceLock<Pool> = OnceLock::new();

pub(super) async fn spawn(
    script: &str,
    workdir: &Path,
    shell: &Shell,
    login: bool,
    environment: &[(OsString, OsString)],
) -> Option<process::SpawnedProcess> {
    if script.contains('\0') {
        return None;
    }
    let (moved, base): (Vec<_>, Vec<_>) = environment
        .iter()
        .cloned()
        .partition(|(name, _)| SessionEnvironment::controls(name));
    let mut input = Vec::with_capacity(script.len() + 128);
    for (name, value) in &moved {
        input.extend_from_slice(name.as_bytes());
        input.push(b'=');
        input.extend_from_slice(value.as_bytes());
        input.push(0);
    }
    input.push(0);
    input.extend_from_slice(script.as_bytes());
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
        drop(nanocode_platform::spawn_blocking(move || {
            evicted.into_iter().chain(discarded).for_each(Idle::kill);
            if fill {
                pool.fill(&refill);
                refill.filling.store(false, Ordering::Release);
            }
        }));
    }
    taken?.start(&input).await.ok()
}

fn identity(directory: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(directory)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

struct Idle {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    ready: bool,
    workdir: (u64, u64),
}

impl Idle {
    fn alive(&mut self) -> bool {
        if self.ready {
            return matches!(self.child.try_wait(), Ok(None));
        }
        let mut byte = [0_u8; 1];
        match nanocode_sys::unix::read(&self.stdout, &mut byte) {
            Ok(1) => {
                self.ready = byte[0] == 0;
                self.ready
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => true,
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
        let workdir = identity(&self.workdir);
        let mut alive = Vec::new();
        let mut discarded = Vec::new();
        for entry in self.idle.drain() {
            let Some(mut idle) = entry.take() else {
                continue;
            };
            if Some(idle.workdir) == workdir && idle.alive() {
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
                .try_update(Ordering::Relaxed, Ordering::Relaxed, grow);
        } else if alive.iter().any(|idle| idle.ready) {
            let _ = self
                .depth
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |depth| {
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
        };
        if let Err(error) = nanocode_sys::unix::set_nonblocking(&idle.stdout, true) {
            idle.kill();
            return Err(error);
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
            args: shell.warm_args(login),
            login,
            workdir: workdir.to_owned(),
            environment: environment.to_vec(),
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
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    (count < depth).then_some(count + 1)
                })
                .is_err()
            {
                return;
            }
            if self
                .total
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
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
