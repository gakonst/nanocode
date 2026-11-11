//! The Unix host: macOS, Linux and Android (ARCH.md "psh-os").
//!
//! - Processes: [`spawn`] (posix_spawn or fork + exec, a session per external, pooled
//!   `waitid(WNOWAIT)` waiters, TERM → grace → KILL on abandon).
//! - Files: [`UnixFile`], whose I/O never waits in the kernel for another part of the
//!   engine ([`ready`]: one readiness thread, `select` on macOS). Pipes to externals are
//!   our private, non-blocking ends; the engine pumps them, so no thread per pump.
//! - FIFO opens, which wait for the other end, run on a helper thread.
//! - Timers on one process-wide thread.
//!
//! Embedded, the host installs no signal handlers and changes no process-wide state.

mod file;
mod fs;
pub mod ready;
mod rlimit;
pub mod signals;
pub mod signum;
mod spawn;

use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::Duration;

use psh_engine::HostFile;
use psh_engine::host::{
    Access, DirEntry, ExtraMetadata, FileKind, Host, HostFuture, Limit, Metadata, OpenOptions,
    Signal, SpawnSpec, Spawned,
};

pub use crate::Leftovers;
pub use file::UnixFile;
pub use spawn::{Placement, settle};

/// Held shared while psh-os makes a descriptor that is not close-on-exec from the
/// start (std on macOS creates pipes with `pipe` + `fcntl`: no `pipe2` there); an
/// embedder that starts processes inheriting its descriptors (std's `Command`) holds it
/// exclusively around the spawn, so no pipe of a shell leaks into such a process
/// (D1905). psh-os's own spawns need not: they close every stray descriptor.
pub static FD_LOCK: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// A pipe, close-on-exec, made under [`FD_LOCK`].
pub fn pipe() -> io::Result<(io::PipeReader, io::PipeWriter)> {
    let _guard = FD_LOCK.read().unwrap_or_else(|e| e.into_inner());
    io::pipe()
}

use spawn::Shared;

/// Settings of a [`UnixHost`].
#[derive(Clone, Copy, Debug)]
pub struct UnixConfig {
    pub placement: Placement,
    /// Time between TERM and KILL when a running external is abandoned.
    pub grace: Duration,
    /// `$$`, when it must not be the process's own id (an embedder running shells
    /// in-process, where `kill $$` must not reach the embedder).
    pub pid: Option<u32>,
    /// What happens to the run's leftovers (released background jobs and what
    /// externals left running) when the host is dropped.
    pub leftovers: Leftovers,
    /// Signals the process ignores when the host is made stay ignored in externals, as
    /// bash keeps the signals ignored on its entry (D1304). On for the standalone `psh`;
    /// off by default: an embedder's own dispositions (a daemon started under `nohup`,
    /// a test run as a background job) are not the shell's, and every signal then
    /// starts at its default unless the script ignores it.
    pub inherit_ignored: bool,
    /// Detached externals (`nohup`, `setsid`) outlive the run. Off for a test runner,
    /// where nothing a case started may survive it: they are killed with the run's
    /// leftovers then (0030).
    pub detached_survive: bool,
}

impl UnixConfig {
    pub const DEFAULT_GRACE: Duration = Duration::from_secs(2);
}

impl Default for UnixConfig {
    fn default() -> Self {
        UnixConfig {
            placement: Placement::Session,
            grace: UnixConfig::DEFAULT_GRACE,
            pid: None,
            leftovers: Leftovers::Kill,
            inherit_ignored: false,
            detached_survive: true,
        }
    }
}

/// The host. One per shell: everything it spawned is that shell's run, and dropping
/// it ends the run ([`Leftovers`]).
pub struct UnixHost {
    shared: Arc<Shared>,
    pid: Option<u32>,
    leftovers: Leftovers,
}

/// Kills a run's leftovers from any thread ([`UnixHost::killer`]).
#[derive(Clone)]
pub struct RunKiller(Arc<Shared>);

impl RunKiller {
    pub fn kill_all(&self) {
        self.0.kill_all();
    }

    /// The run's groups that still have a live member (zombies included), as
    /// [`UnixHost::leftovers`]; usable after the host is gone (leak checks).
    pub fn leftovers(&self) -> Vec<i32> {
        let mut scopes = self.0.scopes.lock().unwrap_or_else(|e| e.into_inner());
        scopes.retain(|s| s.alive());
        scopes.iter().map(|s| s.id()).collect()
    }
}

impl Drop for UnixHost {
    fn drop(&mut self) {
        if self.leftovers == Leftovers::Kill {
            self.shared.kill_all();
        }
    }
}

impl Default for UnixHost {
    fn default() -> Self {
        UnixHost::new(UnixConfig::default())
    }
}

impl UnixHost {
    pub fn new(cfg: UnixConfig) -> UnixHost {
        UnixHost {
            shared: Arc::new(Shared {
                grace: cfg.grace,
                placement: cfg.placement,
                scopes: Mutex::new(Vec::new()),
                ignored_at_start: if cfg.inherit_ignored {
                    spawn::ignored_now()
                } else {
                    // Not Default: arrays implement it only up to 32 (Linux has 65).
                    [false; spawn::NSIG as usize]
                },
                detached_survive: cfg.detached_survive,
                fork_only: Default::default(),
            }),
            pid: cfg.pid,
            leftovers: cfg.leftovers,
        }
    }

    /// Kills the run's leftovers now (see [`Leftovers::Kill`]): for an embedder that
    /// ends a run without dropping the host. Returns at once; [`settle`] waits.
    pub fn kill_all(&self) {
        self.shared.kill_all();
    }

    /// A handle that kills the run's leftovers from any thread (the host itself stays
    /// on the shell's thread).
    pub fn killer(&self) -> RunKiller {
        RunKiller(self.shared.clone())
    }

    /// Every process group of the run recorded so far, live or not (tests check them
    /// after the host is gone).
    pub fn groups(&self) -> Vec<i32> {
        let scopes = self.shared.scopes.lock().unwrap_or_else(|e| e.into_inner());
        scopes.iter().map(|s| s.id()).collect()
    }

    /// The process groups (or, under [`Placement::Inherit`], processes) this host
    /// spawned that still have a live member, zombies included; released externals are
    /// not counted. Empty after a run that leaked nothing, once abandoned externals have
    /// been killed and reaped (up to the grace period).
    pub fn leftovers(&self) -> Vec<i32> {
        let mut scopes = self.shared.scopes.lock().unwrap_or_else(|e| e.into_inner());
        scopes.retain(|s| s.alive());
        scopes.iter().map(|s| s.id()).collect()
    }
}

fn path(b: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(b))
}

fn ready<T: 'static>(v: T) -> HostFuture<T> {
    Box::pin(std::future::ready(v))
}

fn metadata(m: &std::fs::Metadata) -> Metadata {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let ft = m.file_type();
    let kind = if ft.is_symlink() {
        FileKind::Symlink
    } else if ft.is_dir() {
        FileKind::Dir
    } else if ft.is_file() {
        FileKind::File
    } else if ft.is_fifo() {
        FileKind::Fifo
    } else if ft.is_socket() {
        FileKind::Socket
    } else if ft.is_block_device() {
        FileKind::BlockDevice
    } else if ft.is_char_device() {
        FileKind::CharDevice
    } else {
        FileKind::Other
    };
    Metadata {
        kind,
        size: m.len(),
        mode: m.mode() & 0o7777,
        mtime_ns: m.mtime().saturating_mul(1_000_000_000) + m.mtime_nsec(),
        dev: m.dev(),
        ino: m.ino(),
        uid: m.uid(),
        gid: m.gid(),
    }
}

/// `faccessat` with the effective ids, as `test -r/-w/-x` checks.
fn eaccess(p: &[u8], mode: libc::c_int) -> io::Result<bool> {
    let c = CString::new(p).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: a valid NUL-terminated path; no other pointers.
    let rc = unsafe { libc::faccessat(libc::AT_FDCWD, c.as_ptr(), mode, libc::AT_EACCESS) };
    if rc == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EROFS) | Some(libc::ETXTBSY) => Ok(false),
        _ => Err(e),
    }
}

/// Opens `p`. With `opts.mode` and `create`, a file the open creates gets exactly those
/// bits: it is created exclusively (`open` would AND them with the process's umask,
/// which a shell with a looser mask of its own must not inherit), then `fchmod`ed; an
/// existing file is opened as it is and keeps its bits. The process's umask is never
/// touched.
fn open_file(p: &std::path::Path, opts: OpenOptions) -> io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let Some(mode) = opts.mode.filter(|_| opts.create) else {
        return std_open_options(opts).open(p);
    };
    loop {
        let mut create = std_open_options(OpenOptions {
            truncate: false,
            ..opts
        });
        create.create_new(true).mode(mode);
        match create.open(p) {
            Ok(f) => {
                f.set_permissions(std::fs::Permissions::from_mode(mode & 0o7777))?;
                return Ok(f);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && !opts.create_new => {
                let existing = std_open_options(OpenOptions {
                    create: false,
                    ..opts
                });
                match existing.open(p) {
                    // Removed in between: create it after all.
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    r => return r,
                }
            }
            Err(e) => return Err(e),
        }
    }
}

fn std_open_options(opts: OpenOptions) -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.read(opts.read)
        .write(opts.write && !opts.append)
        .append(opts.append)
        .truncate(opts.truncate)
        .create(opts.create && !opts.create_new)
        .create_new(opts.create_new);
    o
}

/// Opens a FIFO on a helper thread: the open waits for the other end, which may be a
/// vproc of the same engine. Dropping the future while the open waits opens the other
/// end briefly (non-blocking), so the thread finishes and closes what it got.
fn open_fifo(p: Vec<u8>, opts: OpenOptions) -> HostFuture<io::Result<Rc<dyn HostFile>>> {
    #[derive(Default)]
    struct Slot {
        result: Option<io::Result<std::fs::File>>,
        waker: Option<Waker>,
        cancelled: bool,
        /// The engine took the result.
        taken: bool,
    }
    let slot = Arc::new(Mutex::new(Slot::default()));
    let s = slot.clone();
    let tp = p.clone();
    let started = crate::spawn_helper("psh-fifo", 64 << 10, move || {
        let r = std_open_options(opts).open(path(&tp));
        let mut st = s.lock().unwrap_or_else(|e| e.into_inner());
        if st.cancelled {
            return;
        }
        st.result = Some(r);
        if let Some(w) = st.waker.take() {
            w.wake();
        }
    });
    if let Err(e) = started {
        return ready(Err(e));
    }
    struct Open {
        slot: Arc<Mutex<Slot>>,
        path: Vec<u8>,
        reading: bool,
    }
    impl Drop for Open {
        fn drop(&mut self) {
            let mut st = self.slot.lock().unwrap_or_else(|e| e.into_inner());
            if st.result.is_some() || st.taken {
                return;
            }
            st.cancelled = true;
            drop(st);
            let flags = if self.reading {
                libc::O_WRONLY
            } else {
                libc::O_RDONLY
            };
            let _ = std::fs::OpenOptions::new()
                .read(!self.reading)
                .write(self.reading)
                .custom_flags(flags | libc::O_NONBLOCK)
                .open(path(&self.path));
        }
    }
    use std::os::unix::fs::OpenOptionsExt;
    let open = Open {
        slot,
        path: p,
        reading: opts.read,
    };
    Box::pin(std::future::poll_fn(move |cx| {
        let mut st = open.slot.lock().unwrap_or_else(|e| e.into_inner());
        match st.result.take() {
            Some(r) => {
                st.taken = true;
                Poll::Ready(r.map(|f| Rc::new(UnixFile::new(f)) as Rc<dyn HostFile>))
            }
            None => {
                st.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }))
}

impl Host for UnixHost {
    fn native_paths(&self) -> Option<psh_engine::host::NativePathFn> {
        Some(|p| Ok(path(p)))
    }

    fn open(&self, p: &[u8], opts: OpenOptions) -> HostFuture<io::Result<Rc<dyn HostFile>>> {
        // Read-write opens of a FIFO never wait.
        if opts.read != opts.write
            && std::fs::metadata(path(p)).is_ok_and(|m| {
                use std::os::unix::fs::FileTypeExt;
                m.file_type().is_fifo()
            })
        {
            return open_fifo(p.to_vec(), opts);
        }
        let r = open_file(&path(p), opts).map(|f| Rc::new(UnixFile::new(f)) as Rc<dyn HostFile>);
        ready(r)
    }

    fn stat(&self, p: &[u8], follow: bool) -> HostFuture<io::Result<Metadata>> {
        let m = if follow {
            std::fs::metadata(path(p))
        } else {
            std::fs::symlink_metadata(path(p))
        };
        ready(m.map(|m| metadata(&m)))
    }

    fn access(&self, p: &[u8], mode: Access) -> HostFuture<io::Result<bool>> {
        let m = match mode {
            Access::Read => libc::R_OK,
            Access::Write => libc::W_OK,
            Access::Execute => libc::X_OK,
        };
        ready(eaccess(p, m))
    }

    fn read_dir(&self, p: &[u8]) -> HostFuture<io::Result<Vec<DirEntry>>> {
        let r = std::fs::read_dir(path(p)).map(|it| {
            it.filter_map(Result::ok)
                .map(|e| DirEntry {
                    name: e.file_name().into_vec(),
                })
                .collect()
        });
        ready(r)
    }

    fn canonicalize(&self, p: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        ready(std::fs::canonicalize(path(p)).map(|p| OsString::from(p).into_vec()))
    }

    fn stat_extra(
        &self,
        p: &[u8],
        follow: bool,
    ) -> HostFuture<io::Result<(Metadata, ExtraMetadata)>> {
        let m = if follow {
            std::fs::metadata(path(p))
        } else {
            std::fs::symlink_metadata(path(p))
        };
        ready(m.map(|m| (metadata(&m), fs::extra(&m))))
    }

    fn mkdir(&self, p: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        ready(fs::mkdir(p, mode))
    }

    fn remove_file(&self, p: &[u8]) -> HostFuture<io::Result<()>> {
        ready(fs::remove_file(p))
    }

    fn remove_dir(&self, p: &[u8]) -> HostFuture<io::Result<()>> {
        ready(fs::remove_dir(p))
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> HostFuture<io::Result<()>> {
        ready(fs::rename(from, to))
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        ready(fs::symlink(target, link))
    }

    fn hard_link(&self, existing: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        ready(fs::hard_link(existing, link))
    }

    fn read_link(&self, p: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        ready(fs::read_link(p))
    }

    fn set_mode(&self, p: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        ready(fs::set_mode(p, mode))
    }

    fn set_times(
        &self,
        p: &[u8],
        atime_ns: Option<i64>,
        mtime_ns: Option<i64>,
        follow: bool,
    ) -> HostFuture<io::Result<()>> {
        ready(fs::set_times(p, atime_ns, mtime_ns, follow))
    }

    fn user_name(&self, uid: u32) -> Option<Vec<u8>> {
        fs::user_name(uid)
    }

    fn group_name(&self, gid: u32) -> Option<Vec<u8>> {
        fs::group_name(gid)
    }

    fn utc_offset(&self, epoch_secs: i64) -> i32 {
        fs::utc_offset(epoch_secs)
    }

    fn resolve(&self, name: &[u8], path_var: &[u8], cwd: &[u8]) -> HostFuture<Option<Vec<u8>>> {
        for dir in path_var.split(|&b| b == b':') {
            let mut cand = if dir.first() == Some(&b'/') {
                dir.to_vec()
            } else {
                // An empty or relative entry is relative to the cwd.
                let mut abs = cwd.to_vec();
                if !dir.is_empty() {
                    abs.push(b'/');
                    abs.extend_from_slice(dir);
                }
                abs
            };
            cand.push(b'/');
            cand.extend_from_slice(name);
            let is_file = std::fs::metadata(path(&cand)).is_ok_and(|m| m.is_file());
            if is_file && eaccess(&cand, libc::X_OK).unwrap_or(false) {
                return ready(Some(cand));
            }
        }
        ready(None)
    }

    fn spawn(&self, spec: SpawnSpec) -> io::Result<Spawned> {
        spawn::spawn(&self.shared, spec)
    }

    fn pipe(&self) -> io::Result<(Rc<dyn HostFile>, Rc<dyn HostFile>)> {
        let (r, w) = pipe()?;
        Ok((Rc::new(UnixFile::new(r)), Rc::new(UnixFile::new(w))))
    }

    fn kill(&self, pid: i32, sig: Signal) -> io::Result<()> {
        // psh numbers signals as Linux does everywhere (D1904); one this OS lacks
        // (SIGSTKFLT, SIGPWR and the real-time ones on macOS) cannot be sent.
        let Some(sig) = signum::to_os(sig) else {
            return Err(io::ErrorKind::Unsupported.into());
        };
        // Embedded (a fake `$$`), the process and its group are the embedder's: a
        // script must not reach them (`kill 0`, `kill -1` and the real pid included).
        if self.pid.is_some() {
            // SAFETY: plain syscall without pointers.
            let own_group = unsafe { libc::getpgrp() };
            let me = std::process::id() as i32;
            if pid == 0 || pid == -1 || pid == me || pid == -own_group {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
        }
        kill(pid, sig)
    }

    fn info(&self) -> psh_engine::host::HostInfo {
        info(self.pid.is_some())
    }

    fn umask(&self) -> u32 {
        spawn::process_umask()
    }

    fn standard_path(&self) -> Vec<u8> {
        let mut buf = vec![0u8; 256];
        loop {
            // SAFETY: `buf` is writable for `buf.len()` bytes; confstr writes at most that
            // many (NUL included) and returns the length the whole value needs.
            let n = unsafe { libc::confstr(libc::_CS_PATH, buf.as_mut_ptr().cast(), buf.len()) };
            if n == 0 {
                return b"/usr/bin:/bin:/usr/sbin:/sbin".to_vec();
            }
            if n <= buf.len() {
                buf.truncate(n - 1);
                return buf;
            }
            buf.resize(n, 0);
        }
    }

    fn rlimit(&self, resource: psh_engine::host::Rlimit) -> Option<(Limit, Limit)> {
        rlimit::get(resource)
    }

    fn now(&self) -> Duration {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
    }

    fn sleep(&self, duration: Duration) -> HostFuture<()> {
        crate::timer::sleep(duration)
    }

    fn pid(&self) -> u32 {
        self.pid.unwrap_or_else(std::process::id)
    }

    fn home_dir(&self, user: &[u8]) -> Option<Vec<u8>> {
        let name = CString::new(user).ok()?;
        let mut buf = vec![0u8; 16 * 1024];
        // SAFETY: an all-zero `passwd` is a valid value of the plain C struct.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: `name` is NUL-terminated; `pwd`, `buf` and `found` are valid for the
        // call and `buf.len()` is the buffer's size.
        let rc = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                &mut pwd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut found,
            )
        };
        if rc != 0 || found.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        // SAFETY: on success `pw_dir` points to a NUL-terminated string inside `buf`,
        // which is alive here.
        let dir = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) };
        Some(dir.to_bytes().to_vec())
    }
}

/// The process and machine facts (D1301). Embedded (`fake_pid`), the shell's parent is
/// the process it runs in.
fn info(fake_pid: bool) -> psh_engine::host::HostInfo {
    // SAFETY: plain calls without pointers.
    let (uid, euid, gid, ppid) = unsafe {
        (
            libc::getuid(),
            libc::geteuid(),
            libc::getgid(),
            libc::getppid(),
        )
    };
    let mut groups = vec![gid];
    // SAFETY: a zero-length query returns the count; the second call fills at most
    // `n` entries of a buffer that long.
    unsafe {
        let n = libc::getgroups(0, std::ptr::null_mut());
        if n > 0 {
            let mut buf = vec![0 as libc::gid_t; n as usize];
            let got = libc::getgroups(n, buf.as_mut_ptr());
            if got > 0 {
                buf.truncate(got as usize);
                groups = buf;
                if !groups.contains(&gid) {
                    groups.insert(0, gid);
                }
            }
        }
    }
    let mut hostname = vec![0u8; 256];
    // SAFETY: the buffer is writable for its length.
    let rc = unsafe { libc::gethostname(hostname.as_mut_ptr().cast(), hostname.len()) };
    if rc == 0 {
        let end = hostname
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(hostname.len());
        hostname.truncate(end);
    } else {
        hostname = b"localhost".to_vec();
    }
    let arch = std::env::consts::ARCH.to_string();
    let (ostype, machtype) = os_names(&arch);
    psh_engine::host::HostInfo {
        uid,
        euid,
        groups,
        ppid: Some(if fake_pid {
            std::process::id()
        } else {
            ppid as u32
        }),
        hostname,
        ostype,
        machtype,
        hosttype: arch,
    }
}

/// `OSTYPE` and `MACHTYPE` as bash reports them, from the running kernel.
fn os_names(arch: &str) -> (String, String) {
    if cfg!(target_os = "macos") {
        let release = uname_release();
        let os = format!("darwin{release}");
        let mach = format!("{arch}-apple-{os}");
        (os, mach)
    } else if cfg!(target_os = "linux") {
        let libc_name = if cfg!(target_env = "musl") {
            "musl"
        } else {
            "gnu"
        };
        let os = format!("linux-{libc_name}");
        let vendor = if arch == "x86_64" { "pc" } else { "unknown" };
        (os.clone(), format!("{arch}-{vendor}-{os}"))
    } else if cfg!(target_os = "android") {
        (
            "linux-android".into(),
            format!("{arch}-unknown-linux-android"),
        )
    } else {
        let os = std::env::consts::OS.to_string();
        let mach = format!("{arch}-unknown-{os}");
        (os, mach)
    }
}

/// The kernel release (`uname -r`).
fn uname_release() -> String {
    // SAFETY: utsname is plain data, filled by uname; its fields are NUL-terminated.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&raw mut u) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(u.release.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// Sends the OS's signal `sig` to process `pid` (negative: a process group), as
/// `kill(2)`.
pub fn kill(pid: i32, sig: libc::c_int) -> io::Result<()> {
    // SAFETY: plain syscall without pointers.
    if unsafe { libc::kill(pid, sig) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests;
