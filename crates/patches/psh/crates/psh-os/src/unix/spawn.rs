//! Spawning, waiting for and abandoning external processes (ARCH.md "psh-os").
//!
//! **Spawn.** `posix_spawn` (macOS; Linux with glibc and `addchdir_np`, which glibc
//! 2.29 added) with the child's descriptors installed by `adddup2`, its cwd by
//! `addchdir_np`, its placement by `POSIX_SPAWN_SETSID` (a session, hence a process
//! group, of its own) unless the host inherits its group, every catchable signal at its
//! default disposition (`SETSIGDEF`, which also undoes the `SIGPIPE` that Rust programs
//! ignore), an empty signal mask, and on macOS `POSIX_SPAWN_CLOEXEC_DEFAULT`, so no
//! descriptor of ours leaks into the child. Elsewhere, a bare `fork` + `execve` whose
//! child does the same with async-signal-safe calls and reports a failed `execve`
//! through a close-on-exec pipe.
//!
//! **Wait.** A pooled waiter thread per running external blocks in
//! `waitid(WEXITED | WSTOPPED | WNOWAIT)`: it observes the exit without reaping, wakes the
//! engine, and the process stays a zombie while its `ExtProc` exists, so a signal to its
//! group never reaches a recycled id (research/execution-core.md §5, pitfalls 3 and 6). No
//! SIGCHLD handler. A stop is consumed and reported through `poll_stop`.
//!
//! **Abandon.** Dropping the `ExtProc` of a running process sends TERM (and CONT, for a
//! stopped group) to its group, and an escalation thread sends KILL to the group when
//! the leader exits or after the grace period, then reaps; see [`crate::lifecycle`].
//!
//! The recipe follows oh-my-pi's `pi-shell/src/sys/spawn` (MIT, see NOTICE).

use std::ffi::CString;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use psh_engine::HostFile;
use psh_engine::host::{ChildFd, ExitStatus, ExtProc, Signal, SpawnSpec, Spawned};

use super::UnixFile;
use crate::lifecycle::{Dropped, Owner, Tracker};

/// Where an external lives relative to the host's process group and session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Every external leads a new session (`setsid`), hence its own process group,
    /// without a controlling terminal: signals and abandonment reach its whole tree, and
    /// it cannot touch the terminal's foreground. The default (agents, embedding).
    Session,
    /// Externals stay in the host's group and session, as with a non-interactive bash:
    /// for the standalone shell, so a group kill of the shell reaches everything it
    /// started (D1067). Signals and abandonment then reach only the external's own pid.
    Inherit,
}

/// The highest signal number the child's dispositions are reset for.
#[cfg(target_os = "linux")]
pub(crate) const NSIG: i32 = 65;
#[cfg(not(target_os = "linux"))]
pub(crate) const NSIG: i32 = 32;

/// How the child starts besides its descriptors.
pub(crate) struct Setup {
    /// Lead a session of its own.
    session: bool,
    group: bool,
    /// Signals that start ignored (by number); every other one starts at its default.
    ignore: [bool; NSIG as usize],
    /// The child's umask, when it differs from the process's.
    umask: Option<libc::mode_t>,
    /// Resource limits the child starts with (`ulimit`).
    rlimits: Vec<(super::rlimit::Resource, libc::rlimit)>,
}

/// The signals the process ignores now, except SIGPIPE (Rust programs ignore it before
/// `main`, so it says nothing about how the shell was started) and SIGCHLD (it concerns
/// our waiting, not the child). Read when a host is made: as with bash, a signal ignored
/// when the shell starts stays ignored in what it runs.
pub(crate) fn ignored_now() -> [bool; NSIG as usize] {
    let mut out = [false; NSIG as usize];
    for (n, slot) in out.iter_mut().enumerate().skip(1) {
        let n = n as i32;
        if n == libc::SIGPIPE || n == libc::SIGCHLD || n == libc::SIGKILL || n == libc::SIGSTOP {
            continue;
        }
        // SAFETY: sigaction is plain data, valid zeroed.
        let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: a null new action only queries; `old` is writable.
        let rc = unsafe { libc::sigaction(n, std::ptr::null(), &raw mut old) };
        *slot = rc == 0 && old.sa_sigaction == libc::SIG_IGN;
    }
    out
}

/// The process's umask, read once without changing it (other threads may be creating
/// files): Linux from `/proc/self/status`; elsewhere a forked child reads it with
/// `umask` (which sets it, in the child only) and reports it through a pipe.
pub(crate) fn process_umask() -> u32 {
    static UMASK: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Ok(status) = std::fs::read_to_string("/proc/self/status")
            && let Some(line) = status.lines().find(|l| l.starts_with("Umask:"))
            && let Ok(m) = u32::from_str_radix(line[6..].trim(), 8)
        {
            return m;
        }
        umask_probe().unwrap_or(0o022)
    });
    *UMASK
}

pub(crate) fn umask_probe() -> Option<u32> {
    let (mut r, w) = super::pipe().ok()?;
    let wfd = w.as_raw_fd();
    // SAFETY: the child calls only async-signal-safe functions (umask, write, _exit)
    // and never returns.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: as above (the forked child).
        unsafe {
            let m = libc::umask(0) as u32;
            let b = m.to_ne_bytes();
            libc::write(wfd, b.as_ptr().cast(), b.len());
            libc::_exit(0);
        }
    }
    drop(w);
    if pid < 0 {
        return None;
    }
    let mut b = [0u8; 4];
    let ok = r.read_exact(&mut b).is_ok();
    reap(pid);
    ok.then(|| u32::from_ne_bytes(b))
}

/// Waiter and escalation threads only wait and signal.
const SMALL_STACK: usize = 64 << 10;

/// What group-wide operations of one external reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The session (and group) it leads.
    Group(i32),
    /// Only the process itself (`Placement::Inherit`).
    Process(i32),
}

impl Scope {
    /// Sends `sig`. Only while the external is unreaped: its id cannot be recycled.
    fn signal(self, sig: Signal) -> io::Result<()> {
        let rc = match self {
            // SAFETY: plain syscall without pointers.
            Scope::Group(g) => match unsafe { libc::killpg(g, sig) } {
                // The group may not exist yet if the child has not run setsid; the
                // leader is its only member then.
                // SAFETY: as above.
                -1 if errno() == libc::ESRCH => unsafe { libc::kill(g, sig) },
                rc => rc,
            },
            // SAFETY: as above.
            Scope::Process(p) => unsafe { libc::kill(p, sig) },
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Whether a process is left in the scope (zombies included).
    pub(crate) fn alive(self) -> bool {
        let target = match self {
            Scope::Group(g) => -g,
            Scope::Process(p) => p,
        };
        // SAFETY: signal 0 only checks for existence and permission.
        let rc = unsafe { libc::kill(target, 0) };
        rc == 0 || errno() == libc::EPERM
    }

    pub(crate) fn id(self) -> i32 {
        match self {
            Scope::Group(g) | Scope::Process(g) => g,
        }
    }
}

/// Spawner state shared by a host and the processes it spawned.
pub(crate) struct Shared {
    pub(crate) grace: Duration,
    pub(crate) placement: Placement,
    /// Scopes of the run's externals (all but detached and disowned ones): leak checks
    /// and the leftovers killed at the end.
    pub(crate) scopes: Mutex<Vec<Scope>>,
    /// Signals ignored when the host was made ([`ignored_now`]).
    pub(crate) ignored_at_start: [bool; NSIG as usize],
    /// `UnixConfig::detached_survive`.
    pub(crate) detached_survive: bool,
    /// Tests: take the fork path everywhere.
    pub(crate) fork_only: std::sync::atomic::AtomicBool,
}

impl Shared {
    fn record(&self, s: Scope) {
        let mut scopes = lock(&self.scopes);
        if scopes.len() >= 256 {
            scopes.retain(|s| s.alive());
        }
        if !scopes.contains(&s) {
            scopes.push(s);
        }
    }

    fn unrecord(&self, s: Scope) {
        lock(&self.scopes).retain(|x| *x != s);
    }

    /// Kills what is left of the run: TERM and CONT to every recorded group with a live
    /// member now, KILL to those still alive after the grace period (on a thread,
    /// counted for [`settle`]). Process scopes (`Placement::Inherit`) are skipped: their
    /// group is the shell's own, not ours to kill. A group whose leader was reaped is
    /// signalled only while it still has members; its id cannot be reused before it is
    /// empty, and the check comes right before the signal.
    pub(crate) fn kill_all(&self) {
        let groups: Vec<i32> = lock(&self.scopes)
            .iter()
            .filter_map(|s| match s {
                Scope::Group(g) if s.alive() => Some(*g),
                _ => None,
            })
            .collect();
        if groups.is_empty() {
            return;
        }
        for &g in &groups {
            let _ = Scope::Group(g).signal(libc::SIGTERM);
            let _ = Scope::Group(g).signal(libc::SIGCONT);
        }
        let grace = self.grace;
        let fallback = groups.clone();
        *lock(&ESCALATING) += 1;
        let started = crate::spawn_helper("psh-kill", Some(SMALL_STACK), move || {
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline && groups.iter().any(|&g| Scope::Group(g).alive()) {
                std::thread::sleep(Duration::from_millis(10));
            }
            for &g in &groups {
                if Scope::Group(g).alive() {
                    let _ = Scope::Group(g).signal(libc::SIGKILL);
                }
            }
            *lock(&ESCALATING) -= 1;
            SETTLED.notify_all();
        });
        if started.is_err() {
            *lock(&ESCALATING) -= 1;
            for &g in &fallback {
                let _ = Scope::Group(g).signal(libc::SIGKILL);
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.to_string())
}

fn cstring(b: &[u8]) -> io::Result<CString> {
    CString::new(b).map_err(|_| invalid("argument contains a NUL byte"))
}

/// The C strings of an exec, built before the spawn.
struct Exec {
    path: CString,
    cwd: CString,
    _argv: Vec<CString>,
    _env: Vec<CString>,
    argv_ptrs: Vec<*mut libc::c_char>,
    env_ptrs: Vec<*mut libc::c_char>,
}

impl Exec {
    fn new(
        program: &[u8],
        argv: &[Vec<u8>],
        env: &[(Vec<u8>, Vec<u8>)],
        cwd: &[u8],
    ) -> io::Result<Exec> {
        let argv = argv
            .iter()
            .map(|a| cstring(a))
            .collect::<io::Result<Vec<_>>>()?;
        let env = env
            .iter()
            .map(|(k, v)| {
                let mut kv = Vec::with_capacity(k.len() + v.len() + 1);
                kv.extend_from_slice(k);
                kv.push(b'=');
                kv.extend_from_slice(v);
                cstring(&kv)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let ptrs = |v: &[CString]| -> Vec<*mut libc::c_char> {
            v.iter()
                .map(|c| c.as_ptr().cast_mut())
                .chain(std::iter::once(std::ptr::null_mut()))
                .collect()
        };
        Ok(Exec {
            path: cstring(program)?,
            cwd: cstring(cwd)?,
            argv_ptrs: ptrs(&argv),
            env_ptrs: ptrs(&env),
            _argv: argv,
            _env: env,
        })
    }
}

/// A source descriptor for the child's descriptors.
enum Src {
    /// Ours, closed after the spawn so the pumps see the child's end of file.
    Owned(OwnedFd),
    /// A file of the engine, kept open by the engine during the spawn.
    Borrowed(RawFd),
}

impl Src {
    fn raw(&self) -> RawFd {
        match self {
            Src::Owned(f) => f.as_raw_fd(),
            Src::Borrowed(f) => *f,
        }
    }
}

/// Duplicates `fd` to a close-on-exec descriptor at or above `floor`.
fn dup_above(fd: RawFd, floor: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl only reads the descriptor table; a success returns a new
    // descriptor nobody else owns.
    let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, floor) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `n` is a fresh descriptor we own.
    Ok(unsafe { OwnedFd::from_raw_fd(n) })
}

pub(crate) fn spawn(shared: &Arc<Shared>, mut spec: SpawnSpec) -> io::Result<Spawned> {
    let exec = Exec::new(&spec.program, &spec.argv, &spec.env, &spec.cwd)?;
    let mut sources: Vec<Src> = Vec::new();
    // (child fd, index into `sources`)
    let mut installs: Vec<(i32, usize)> = Vec::new();
    let mut held: Vec<Rc<dyn HostFile>> = Vec::new();
    let mut pipes: Vec<(u32, Rc<dyn HostFile>)> = Vec::new();
    let mut null: Option<usize> = None;
    for (n, fd) in std::mem::take(&mut spec.fds) {
        let target = i32::try_from(n).map_err(|_| invalid("descriptor out of range"))?;
        let idx = match fd {
            ChildFd::File(f) => {
                let any: &dyn std::any::Any = &*f;
                let Some(uf) = any.downcast_ref::<UnixFile>() else {
                    return Err(io::Error::other("not a file of this host"));
                };
                sources.push(Src::Borrowed(uf.raw_fd()));
                held.push(f);
                sources.len() - 1
            }
            ChildFd::Pipe { input } => {
                let (r, w) = super::pipe()?;
                let (ours, theirs) = if input {
                    (OwnedFd::from(w), OwnedFd::from(r))
                } else {
                    (OwnedFd::from(r), OwnedFd::from(w))
                };
                pipes.push((n, Rc::new(UnixFile::private(ours)?) as Rc<dyn HostFile>));
                sources.push(Src::Owned(theirs));
                sources.len() - 1
            }
            ChildFd::Dup(m) => {
                let m = i32::try_from(m).map_err(|_| invalid("descriptor out of range"))?;
                match installs.iter().find(|(t, _)| *t == m) {
                    Some((_, i)) => *i,
                    None => return Err(invalid("dup of a descriptor not given")),
                }
            }
            ChildFd::Null => match null {
                Some(i) => i,
                None => {
                    let f = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open("/dev/null")?;
                    sources.push(Src::Owned(f.into()));
                    null = Some(sources.len() - 1);
                    sources.len() - 1
                }
            },
        };
        installs.retain(|(t, _)| *t != target);
        installs.push((target, idx));
    }
    // Every source above every target: installing one target (dup2) never clobbers a
    // source still to be installed.
    let floor = installs.iter().map(|(t, _)| *t).max().unwrap_or(2).max(2) + 1;
    for s in &mut sources {
        if s.raw() < floor {
            *s = Src::Owned(dup_above(s.raw(), floor)?);
        }
    }
    let fds: Vec<(i32, RawFd)> = installs
        .iter()
        .map(|(t, i)| (*t, sources[*i].raw()))
        .collect();
    let session = spec.detach || shared.placement == Placement::Session;
    let mut setup = Setup {
        session,
        group: spec.group && !session,
        ignore: shared.ignored_at_start,
        umask: spec
            .umask
            .filter(|&m| m != process_umask())
            .map(|m| m as libc::mode_t),
        rlimits: super::rlimit::for_child(&spec.rlimits),
    };
    let mut extra_ignored = false;
    // Linux numbers (D1283) to the OS's.
    for sig in spec
        .sig_ignore
        .iter()
        .filter_map(|&s| super::signum::to_os(s))
    {
        if let Some(slot) = usize::try_from(sig)
            .ok()
            .and_then(|n| setup.ignore.get_mut(n))
            && sig != libc::SIGKILL
            && sig != libc::SIGSTOP
        {
            extra_ignored |= !*slot;
            *slot = true;
        }
    }
    // posix_spawn can neither set a umask or resource limits nor ignore a signal the
    // process does not ignore itself.
    let fork = setup.umask.is_some()
        || !setup.rlimits.is_empty()
        || extra_ignored
        || shared.fork_only.load(std::sync::atomic::Ordering::Relaxed);
    let mut tries = 0;
    let pid = loop {
        let r = if fork {
            fork_exec(&exec, &fds, &setup, floor)
        } else {
            launch(&exec, &fds, &setup, floor)
        };
        match r {
            Ok(pid) => break pid,
            // ETXTBSY: another thread's fork still holds a just-written script open
            // for writing until it execs; it passes in microseconds.
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && tries < 200 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
            // A file without `#!` that the kernel refuses: the engine runs it as a psh
            // script, as bash runs it itself (D1300).
            Err(e) if e.raw_os_error() == Some(libc::ENOEXEC) => {
                return Err(psh_engine::host::RunInShell::error());
            }
            Err(e) => return Err(e),
        }
    };
    // Our copies of the child's ends: the pumps see end of file when the child (and
    // what it starts) closes its own.
    drop(sources);
    drop(held);
    let scope = if session || setup.group {
        Scope::Group(pid)
    } else {
        Scope::Process(pid)
    };
    if !spec.detach || !shared.detached_survive {
        shared.record(scope);
    }
    let tracker = Arc::new(Tracker::new());
    if let Err(e) = watch(pid, tracker.clone()) {
        // Without a waiter nobody would observe the exit: undo the spawn.
        let _ = scope.signal(libc::SIGKILL);
        reap(pid);
        shared.unrecord(scope);
        return Err(e);
    }
    Ok(Spawned {
        proc: Box::new(UnixProc {
            pid,
            scope,
            tracker,
            shared: shared.clone(),
            detached: spec.detach,
        }),
        pipes,
    })
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
fn launch(exec: &Exec, fds: &[(i32, RawFd)], setup: &Setup, floor: RawFd) -> io::Result<i32> {
    match posix::add_chdir() {
        Some(add_chdir) => posix::spawn(exec, fds, setup, add_chdir),
        None => fork_exec(exec, fds, setup, floor),
    }
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
fn launch(exec: &Exec, fds: &[(i32, RawFd)], setup: &Setup, floor: RawFd) -> io::Result<i32> {
    fork_exec(exec, fds, setup, floor)
}

/// Closes, in a forked child, every descriptor not kept: on macOS from the process's own
/// descriptor list (`proc_pidinfo`, a plain system call, into a buffer allocated before
/// the fork), on Linux with `close_range` over the gaps between the kept ones.
struct Strays {
    #[cfg(target_os = "macos")]
    buf: Vec<libc::proc_fdinfo>,
}

impl Strays {
    /// Before the fork: room for the descriptors open now, and some more.
    fn prepare() -> Strays {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: with a null buffer proc_pidinfo only returns the size needed.
            let size = unsafe {
                libc::proc_pidinfo(
                    libc::getpid(),
                    libc::PROC_PIDLISTFDS,
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            let n = usize::try_from(size).unwrap_or(0) / std::mem::size_of::<libc::proc_fdinfo>();
            Strays {
                buf: Vec::with_capacity(n + 256),
            }
        }
        #[cfg(not(target_os = "macos"))]
        Strays {}
    }

    /// In the forked child (async-signal-safe: system calls on memory prepared before
    /// the fork). `keep` is sorted.
    fn close(&mut self, keep: &[RawFd]) {
        #[cfg(target_os = "macos")]
        {
            let cap = self.buf.capacity();
            let bytes = cap * std::mem::size_of::<libc::proc_fdinfo>();
            // Repeated while the buffer comes back full: each pass closes what it lists.
            loop {
                // SAFETY: the buffer has room for `cap` entries; proc_pidinfo writes at
                // most `bytes` bytes and returns how many it wrote.
                let got = unsafe {
                    libc::proc_pidinfo(
                        libc::getpid(),
                        libc::PROC_PIDLISTFDS,
                        0,
                        self.buf.as_mut_ptr().cast(),
                        i32::try_from(bytes).unwrap_or(i32::MAX),
                    )
                };
                let n =
                    usize::try_from(got).unwrap_or(0) / std::mem::size_of::<libc::proc_fdinfo>();
                let mut closed = false;
                for i in 0..n.min(cap) {
                    // SAFETY: proc_pidinfo initialized the first `n` entries.
                    let fd = unsafe { (*self.buf.as_ptr().add(i)).proc_fd };
                    if keep.binary_search(&fd).is_err() {
                        // SAFETY: closing a descriptor of this (forked) process.
                        unsafe { libc::close(fd) };
                        closed = true;
                    }
                }
                if n < cap || !closed {
                    break;
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            let mut from: RawFd = 0;
            for &k in keep.iter().chain(std::iter::once(&RawFd::MAX)) {
                if k > from {
                    let last = if k == RawFd::MAX {
                        u32::MAX
                    } else {
                        (k - 1) as u32
                    };
                    // SAFETY: close_range only closes descriptors of this (forked)
                    // process; an old kernel without it returns ENOSYS, which leaves
                    // things as they were.
                    unsafe { libc::syscall(libc::SYS_close_range, from as u32, last, 0u32) };
                }
                from = k.saturating_add(1);
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = keep;
    }
}

/// The fork path: the child sets itself up with async-signal-safe calls only, on data
/// prepared before the fork, and reports a failed `chdir` or `execve` by writing its
/// errno into a close-on-exec pipe (nothing arrives when the exec succeeded).
fn fork_exec(exec: &Exec, fds: &[(i32, RawFd)], setup: &Setup, floor: RawFd) -> io::Result<i32> {
    let (err_r, err_w) = super::pipe()?;
    // Above every target, so no dup2 in the child closes it.
    let err_w = dup_above(OwnedFd::from(err_w).as_raw_fd(), floor)?;
    let err_fd = err_w.as_raw_fd();
    // The descriptors the child keeps: its installed targets and the error pipe.
    let mut keep: Vec<RawFd> = fds.iter().map(|&(t, _)| t).collect();
    keep.extend([0, 1, 2, err_fd]);
    keep.sort_unstable();
    keep.dedup();
    let mut strays = Strays::prepare();
    // Every signal stays blocked across the fork: the child must not run the host's
    // handlers before its dispositions are reset.
    // SAFETY: sigset_t is plain data, initialized by sigfillset; the calls only read
    // and write the sets passed.
    let old = unsafe {
        let mut all = std::mem::zeroed::<libc::sigset_t>();
        let mut old = std::mem::zeroed::<libc::sigset_t>();
        libc::sigfillset(&raw mut all);
        libc::pthread_sigmask(libc::SIG_BLOCK, &raw const all, &raw mut old);
        old
    };
    // SAFETY: the child calls only async-signal-safe functions (setsid, chdir, dup2,
    // umask, setrlimit, sigaction, sigprocmask, execve, write, _exit) on memory prepared before the fork,
    // and never returns.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: as above (the forked child).
        unsafe {
            let fail = |e: i32| -> ! {
                let b = e.to_ne_bytes();
                libc::write(err_fd, b.as_ptr().cast(), b.len());
                libc::_exit(127)
            };
            if setup.session && libc::setsid() < 0 {
                fail(errno());
            }
            if setup.group && libc::setpgid(0, 0) < 0 {
                fail(errno());
            }
            if libc::chdir(exec.cwd.as_ptr()) != 0 {
                fail(errno());
            }
            for &(target, src) in fds {
                if libc::dup2(src, target) < 0 {
                    fail(errno());
                }
            }
            // A descriptor another thread opened without close-on-exec, or before it
            // could set it (std on macOS makes pipes with `pipe` + `fcntl`), would live
            // on in the program: a pipe kept open that way keeps its reader from ever
            // seeing the end (D1905). posix_spawn has CLOEXEC_DEFAULT for this.
            strays.close(&keep);
            if let Some(m) = setup.umask {
                libc::umask(m);
            }
            // A limit the kernel refuses (macOS caps open files below "unlimited")
            // leaves the inherited one, as a failed `ulimit` would.
            for (res, lim) in &setup.rlimits {
                libc::setrlimit(*res, lim);
            }
            let mut dfl = std::mem::zeroed::<libc::sigaction>();
            dfl.sa_sigaction = libc::SIG_DFL;
            let mut ign = std::mem::zeroed::<libc::sigaction>();
            ign.sa_sigaction = libc::SIG_IGN;
            for sig in 1..NSIG {
                if sig != libc::SIGKILL && sig != libc::SIGSTOP {
                    let act = if setup.ignore[sig as usize] {
                        &ign
                    } else {
                        &dfl
                    };
                    libc::sigaction(sig, act, std::ptr::null_mut());
                }
            }
            let empty = std::mem::zeroed::<libc::sigset_t>();
            libc::sigprocmask(libc::SIG_SETMASK, &raw const empty, std::ptr::null_mut());
            libc::execve(
                exec.path.as_ptr(),
                exec.argv_ptrs.as_ptr().cast(),
                exec.env_ptrs.as_ptr().cast(),
            );
            fail(errno());
        }
    }
    let fork_err = io::Error::last_os_error();
    // SAFETY: restores the mask saved above.
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &raw const old, std::ptr::null_mut()) };
    drop(err_w);
    if pid < 0 {
        return Err(fork_err);
    }
    let mut buf = [0u8; 4];
    let mut got = 0;
    let mut r = err_r;
    while got < 4 {
        match r.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if got == 4 {
        reap(pid);
        return Err(io::Error::from_raw_os_error(i32::from_ne_bytes(buf)));
    }
    Ok(pid)
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
mod posix {
    use std::io;
    use std::mem::MaybeUninit;
    use std::os::fd::RawFd;

    use super::{Exec, NSIG, Setup};

    pub(super) type AddChdir = unsafe extern "C" fn(
        *mut libc::posix_spawn_file_actions_t,
        *const libc::c_char,
    ) -> libc::c_int;

    #[cfg(target_os = "macos")]
    pub(super) fn add_chdir() -> Option<AddChdir> {
        Some(libc::posix_spawn_file_actions_addchdir_np)
    }

    /// glibc 2.29 added it; looked up at run time so older glibcs still load us (and
    /// take the fork path).
    #[cfg(target_os = "linux")]
    pub(super) fn add_chdir() -> Option<AddChdir> {
        static ADD_CHDIR: std::sync::LazyLock<Option<AddChdir>> = std::sync::LazyLock::new(|| {
            // SAFETY: RTLD_DEFAULT and the NUL-terminated name are valid.
            let sym = unsafe {
                libc::dlsym(
                    libc::RTLD_DEFAULT,
                    c"posix_spawn_file_actions_addchdir_np".as_ptr(),
                )
            };
            if sym.is_null() {
                None
            } else {
                // SAFETY: the libc function has exactly this signature and lives
                // as long as the process.
                Some(unsafe { std::mem::transmute::<*mut libc::c_void, AddChdir>(sym) })
            }
        });
        *ADD_CHDIR
    }

    /// `POSIX_SPAWN_SETSID` (`<spawn.h>`, macOS 10.15+; not in the libc crate).
    #[cfg(target_os = "macos")]
    const SETSID: libc::c_int = 0x0400;
    #[cfg(target_os = "linux")]
    const SETSID: libc::c_int = libc::POSIX_SPAWN_SETSID as libc::c_int;

    fn check(rc: libc::c_int) -> io::Result<()> {
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(rc))
        }
    }

    struct Actions(MaybeUninit<libc::posix_spawn_file_actions_t>);

    impl Actions {
        fn new() -> io::Result<Actions> {
            let mut a = MaybeUninit::uninit();
            // SAFETY: init writes the object; only an initialized one is wrapped.
            check(unsafe { libc::posix_spawn_file_actions_init(a.as_mut_ptr()) })?;
            Ok(Actions(a))
        }

        fn ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
            self.0.as_mut_ptr()
        }
    }

    impl Drop for Actions {
        fn drop(&mut self) {
            // SAFETY: initialized in `new`.
            unsafe { libc::posix_spawn_file_actions_destroy(self.0.as_mut_ptr()) };
        }
    }

    struct Attr(MaybeUninit<libc::posix_spawnattr_t>);

    impl Attr {
        fn new() -> io::Result<Attr> {
            let mut a = MaybeUninit::uninit();
            // SAFETY: init writes the object; only an initialized one is wrapped.
            check(unsafe { libc::posix_spawnattr_init(a.as_mut_ptr()) })?;
            Ok(Attr(a))
        }

        fn ptr(&mut self) -> *mut libc::posix_spawnattr_t {
            self.0.as_mut_ptr()
        }
    }

    impl Drop for Attr {
        fn drop(&mut self) {
            // SAFETY: initialized in `new`.
            unsafe { libc::posix_spawnattr_destroy(self.0.as_mut_ptr()) };
        }
    }

    pub(super) fn spawn(
        exec: &Exec,
        fds: &[(i32, RawFd)],
        setup: &Setup,
        add_chdir: AddChdir,
    ) -> io::Result<i32> {
        let mut actions = Actions::new()?;
        for &(target, src) in fds {
            // SAFETY: initialized actions; descriptors are plain integers.
            check(unsafe { libc::posix_spawn_file_actions_adddup2(actions.ptr(), src, target) })?;
        }
        // SAFETY: initialized actions; libc copies the NUL-terminated path.
        check(unsafe { add_chdir(actions.ptr(), exec.cwd.as_ptr()) })?;
        let mut attr = Attr::new()?;
        let mut flags = libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF;
        if setup.session {
            flags |= SETSID;
        }
        if setup.group {
            flags |= libc::POSIX_SPAWN_SETPGROUP;
            // SAFETY: the attribute object is initialized.
            check(unsafe { libc::posix_spawnattr_setpgroup(attr.ptr(), 0) })?;
        }
        #[cfg(target_os = "macos")]
        {
            flags |= libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
        }
        // SAFETY: sigset_t is plain data, initialized by sigemptyset/sigfillset; the
        // attribute object is initialized and copies the sets.
        unsafe {
            let mut empty = std::mem::zeroed::<libc::sigset_t>();
            let mut defaults = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&raw mut empty);
            libc::sigemptyset(&raw mut defaults);
            // Ignored signals stay as the process has them (the caller made sure
            // that is what the child wants); every other one goes to its default.
            for sig in 1..NSIG {
                if sig != libc::SIGKILL && sig != libc::SIGSTOP && !setup.ignore[sig as usize] {
                    libc::sigaddset(&raw mut defaults, sig);
                }
            }
            check(libc::posix_spawnattr_setflags(
                attr.ptr(),
                flags as libc::c_short,
            ))?;
            check(libc::posix_spawnattr_setsigmask(
                attr.ptr(),
                &raw const empty,
            ))?;
            check(libc::posix_spawnattr_setsigdefault(
                attr.ptr(),
                &raw const defaults,
            ))?;
        }
        let mut pid: libc::pid_t = 0;
        // SAFETY: every pointer is valid for the call: NUL-terminated strings and
        // NULL-terminated pointer arrays owned by `exec`, initialized actions and
        // attributes.
        check(unsafe {
            libc::posix_spawn(
                &raw mut pid,
                exec.path.as_ptr(),
                actions.ptr(),
                attr.ptr(),
                exec.argv_ptrs.as_ptr(),
                exec.env_ptrs.as_ptr(),
            )
        })?;
        Ok(pid)
    }
}

/// What the host's threads share, held for a fork (`Host::prepare_fork`): the waiter
/// pool, the escalations, the readiness thread's registrations and the timers. Their
/// threads do not exist in the copy, which starts with all four empty.
pub(crate) struct Prepared {
    shared: Arc<Shared>,
    idle: std::sync::MutexGuard<'static, Vec<Arc<Mailbox>>>,
    escalating: std::sync::MutexGuard<'static, usize>,
    ready: super::ready::Held,
    timers: crate::timer::Held,
}

pub(crate) fn prepare(shared: &Arc<Shared>) -> Prepared {
    Prepared {
        shared: shared.clone(),
        idle: lock(&IDLE),
        escalating: lock(&ESCALATING),
        ready: super::ready::hold(),
        timers: crate::timer::hold(),
    }
}

impl psh_engine::host::PreparedFork for Prepared {
    fn fork(mut self: Box<Self>) -> io::Result<psh_engine::host::Forked> {
        let old = super::signals::block_all_in_thread();
        // SAFETY: fork has no arguments. The copy runs on with this thread alone; what
        // the other threads share is held here and reset there before it is used.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            self.idle.clear();
            *self.escalating = 0;
            self.ready.restart();
            self.timers.restart();
            super::signals::forked();
            lock(&self.shared.scopes).clear();
            drop(self);
            super::signals::restore_mask(old);
            return Ok(psh_engine::host::Forked::Child);
        }
        let failed = (pid < 0).then(io::Error::last_os_error);
        let shared = self.shared.clone();
        drop(self);
        super::signals::restore_mask(old);
        if let Some(e) = failed {
            return Err(e);
        }
        let scope = Scope::Process(pid);
        shared.record(scope);
        let tracker = Arc::new(Tracker::new());
        if let Err(e) = watch(pid, tracker.clone()) {
            let _ = scope.signal(libc::SIGKILL);
            reap(pid);
            shared.unrecord(scope);
            return Err(e);
        }
        Ok(psh_engine::host::Forked::Parent(Box::new(UnixProc {
            pid,
            scope,
            tracker,
            shared,
            detached: false,
        })))
    }
}

/// One waiter thread's slot for its next external.
struct Mailbox {
    job: Mutex<Option<(i32, Arc<Tracker>)>>,
    ready: Condvar,
}

/// Parked waiter threads, shared by every host of the process: reusing one saves a
/// thread creation per external.
static IDLE: Mutex<Vec<Arc<Mailbox>>> = Mutex::new(Vec::new());
const IDLE_MAX: usize = 8;
const IDLE_FOR: Duration = Duration::from_secs(10);

/// Waits for the exit of `pid` on a parked waiter, or a new one.
fn watch(pid: i32, tracker: Arc<Tracker>) -> io::Result<()> {
    let parked = lock(&IDLE).pop();
    if let Some(mb) = parked {
        *lock(&mb.job) = Some((pid, tracker));
        mb.ready.notify_one();
        return Ok(());
    }
    let mb = Arc::new(Mailbox {
        job: Mutex::new(Some((pid, tracker))),
        ready: Condvar::new(),
    });
    crate::spawn_helper("psh-wait", Some(SMALL_STACK), move || waiter(&mb))
}

fn waiter(mb: &Arc<Mailbox>) {
    while let Some((pid, tracker)) = next_job(mb) {
        let status = wait_exit(pid, |sig| {
            if let Some(w) = tracker.stopped(sig) {
                w.wake();
            }
        });
        let waker = tracker.exited(status, || reap(pid));
        drop(tracker);
        if let Some(w) = waker {
            w.wake();
        }
        let mut idle = lock(&IDLE);
        if idle.len() >= IDLE_MAX {
            return;
        }
        idle.push(mb.clone());
    }
}

/// The next external for this waiter; `None` once it left the pool idle.
fn next_job(mb: &Arc<Mailbox>) -> Option<(i32, Arc<Tracker>)> {
    let mut job = lock(&mb.job);
    loop {
        if let Some(next) = job.take() {
            return Some(next);
        }
        let (g, timeout) = mb
            .ready
            .wait_timeout(job, IDLE_FOR)
            .unwrap_or_else(|e| e.into_inner());
        job = g;
        if !timeout.timed_out() || job.is_some() {
            continue;
        }
        // Leave the pool unless a spawner just took this waiter from it (its job is on
        // the way). `watch` locks the pool before a mailbox: unlock ours first.
        drop(job);
        let left = {
            let mut idle = lock(&IDLE);
            let before = idle.len();
            idle.retain(|m| !Arc::ptr_eq(m, mb));
            idle.len() < before
        };
        job = lock(&mb.job);
        if left && job.is_none() {
            return None;
        }
    }
}

/// Blocks until `pid` exits and returns its status, leaving it unreaped.
fn wait_exit(pid: i32, stopped: impl Fn(Signal)) -> ExitStatus {
    loop {
        // SAFETY: siginfo_t is plain data, valid zeroed.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is writable; WNOWAIT leaves the child waitable.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &raw mut info,
                libc::WEXITED | libc::WSTOPPED | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            // SAFETY: waitid filled `info` for a CLD_* event.
            let status = unsafe { info.si_status() };
            match info.si_code {
                libc::CLD_EXITED => return ExitStatus::Exited(status),
                libc::CLD_KILLED | libc::CLD_DUMPED => {
                    return ExitStatus::Signaled(super::signum::from_os(status));
                }
                code => {
                    // With WNOWAIT the report of a stop (or, on macOS, a continue) would
                    // repeat. Consume it without reaping.
                    // SAFETY: as above.
                    let mut seen: libc::siginfo_t = unsafe { std::mem::zeroed() };
                    // SAFETY: `seen` is writable; without WEXITED this never reaps.
                    let rc = unsafe {
                        libc::waitid(
                            libc::P_PID,
                            pid as libc::id_t,
                            &raw mut seen,
                            libc::WSTOPPED | libc::WCONTINUED | libc::WNOHANG,
                        )
                    };
                    // SAFETY: `seen` was zeroed and possibly filled by waitid.
                    if rc != 0 || unsafe { seen.si_pid() } == 0 {
                        std::thread::sleep(Duration::from_millis(10));
                    } else if code == libc::CLD_STOPPED {
                        stopped(super::signum::from_os(status));
                    }
                    continue;
                }
            }
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            // ECHILD: someone else reaped it (SIGCHLD ignored by the embedder). The
            // status is lost.
            return ExitStatus::Exited(127);
        }
    }
}

/// Reaps `pid`, which has exited (or was sent KILL).
fn reap(pid: i32) {
    let mut st = 0;
    // SAFETY: `st` is writable; `pid` is our unreaped child.
    while unsafe { libc::waitpid(pid, &raw mut st, 0) } < 0 && errno() == libc::EINTR {}
}

/// A spawned external. Never reaped while this exists.
struct UnixProc {
    pid: i32,
    scope: Scope,
    tracker: Arc<Tracker>,
    shared: Arc<Shared>,
    /// `SpawnSpec::detach`: dropping it releases instead of abandoning.
    detached: bool,
}

impl ExtProc for UnixProc {
    fn pid(&self) -> Option<u32> {
        u32::try_from(self.pid).ok()
    }

    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<ExitStatus> {
        self.tracker.poll(cx)
    }

    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Signal> {
        self.tracker.poll_stop(cx)
    }

    fn signal(&mut self, sig: Signal) {
        let Some(sig) = super::signum::to_os(sig) else {
            return;
        };
        let mut life = self.tracker.lock();
        if life.reaped() {
            return;
        }
        if sig == libc::SIGTERM {
            life.term_sent();
        }
        let _ = self.scope.signal(sig);
    }

    fn release(self: Box<Self>) {
        // Still part of the run: its group is killed with the leftovers (`Drop`).
        let mut life = self.tracker.lock();
        if Tracker::release(&mut life) {
            reap(self.pid);
            life.set_reaped();
        }
    }

    fn disown(self: Box<Self>) {
        // Its session is no longer the run's: not killed with the leftovers.
        self.shared.unrecord(self.scope);
        self.release();
    }
}

impl Drop for UnixProc {
    fn drop(&mut self) {
        let mut life = self.tracker.lock();
        if life.owner() != Owner::Engine {
            return;
        }
        if self.detached {
            if Tracker::release(&mut life) {
                reap(self.pid);
                life.set_reaped();
            }
            return;
        }
        match Tracker::abandon(&mut life) {
            Dropped::Reap => {
                reap(self.pid);
                life.set_reaped();
            }
            Dropped::Escalate { term } => {
                // Under the lock: the process is unreaped, its scope still ours.
                if term {
                    let _ = self.scope.signal(libc::SIGTERM);
                }
                // A stopped group acts on TERM only once continued.
                let _ = self.scope.signal(libc::SIGCONT);
                drop(life);
                let deadline = Instant::now() + self.shared.grace;
                let (tracker, scope, pid) = (self.tracker.clone(), self.scope, self.pid);
                *lock(&ESCALATING) += 1;
                let started = crate::spawn_helper("psh-kill", Some(SMALL_STACK), move || {
                    escalate(&tracker, scope, pid, deadline);
                    *lock(&ESCALATING) -= 1;
                    SETTLED.notify_all();
                });
                if started.is_err() {
                    *lock(&ESCALATING) -= 1;
                    // No thread: KILL now rather than never; the waiter reaps.
                    let mut life = self.tracker.lock();
                    let _ = self.scope.signal(libc::SIGKILL);
                    if Tracker::release(&mut life) {
                        reap(self.pid);
                        life.set_reaped();
                    }
                }
            }
        }
    }
}

/// Escalations running (abandoned externals not yet killed and reaped), process-wide.
static ESCALATING: Mutex<usize> = Mutex::new(0);
static SETTLED: Condvar = Condvar::new();

/// Waits up to `timeout` until every abandoned external of the process was killed and
/// reaped; whether they were. A standalone shell calls it before it exits, since the
/// escalation threads die with the process.
pub fn settle(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut n = lock(&ESCALATING);
    while *n > 0 {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        n = SETTLED
            .wait_timeout(n, deadline - now)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    true
}

/// KILL the group when its leader exits or the grace period ends, whichever comes
/// first (the unreaped leader keeps the group id ours), then reap. Members still there
/// when the leader is gone had TERM already; they would be orphans (D1065).
fn escalate(tracker: &Tracker, scope: Scope, pid: i32, deadline: Instant) {
    tracker.wait_exit_until(Some(deadline));
    let _ = scope.signal(libc::SIGKILL);
    tracker.wait_exit_until(None);
    let mut life = tracker.lock();
    reap(pid);
    life.set_reaped();
}
