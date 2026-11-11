//! A simple host on the standard library, for simple embedders and tests (utility unit
//! tests run on it); the `psh` binary and psh-spec run on `psh-os`'s hosts but use
//! the helpers here: `drive`/`drive_until` (stepping a shell to its end on the calling
//! thread) and `engine_thread` (a thread with the stack a shell needs).
//!
//! Its limits, which psh-os does not have: std `Command` spawns, a waiter thread per
//! child polling `try_wait` with backoff, helper threads for the pipes between the
//! engine and children; no process groups; `signal` and abandon send KILL (std has no
//! TERM); `access` approximates from permission bits; reads and writes of regular files
//! and of the shell's own stdio are synchronous.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::host::{
    Access, ChildFd, DirEntry, ExitStatus, ExtProc, ExtraMetadata, FileKind, Host, HostFile,
    HostFuture, Metadata, OpenOptions, Signal, SpawnSpec, Spawned,
};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(unix)]
fn os(b: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(b).to_os_string()
}

#[cfg(not(unix))]
fn os(b: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(b).into_owned())
}

#[cfg(unix)]
fn bytes(p: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn bytes(p: &std::ffi::OsStr) -> Vec<u8> {
    p.to_string_lossy().replace('\\', "/").into_bytes()
}

fn path(b: &[u8]) -> PathBuf {
    PathBuf::from(os(b))
}

/// A file of the OS: regular files, `/dev/null`, the shell's own stdio. Reads and
/// writes are done at once (v1, see the module docs).
pub struct StdFile {
    file: File,
}

impl StdFile {
    pub fn new(file: File) -> StdFile {
        StdFile { file }
    }

    /// The process's own descriptor 0, 1 or 2 (a duplicate).
    pub fn stdio(n: u32) -> io::Result<StdFile> {
        #[cfg(unix)]
        {
            use std::os::fd::AsFd;
            let fd = match n {
                0 => io::stdin().as_fd().try_clone_to_owned()?,
                1 => io::stdout().as_fd().try_clone_to_owned()?,
                _ => io::stderr().as_fd().try_clone_to_owned()?,
            };
            Ok(StdFile::new(File::from(fd)))
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsHandle;
            let h = match n {
                0 => io::stdin().as_handle().try_clone_to_owned()?,
                1 => io::stdout().as_handle().try_clone_to_owned()?,
                _ => io::stderr().as_handle().try_clone_to_owned()?,
            };
            Ok(StdFile::new(File::from(h)))
        }
    }
}

impl HostFile for StdFile {
    fn poll_read(&self, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        loop {
            match (&self.file).read(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                r => return Poll::Ready(r),
            }
        }
    }

    fn poll_write(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            match (&self.file).write(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                r => return Poll::Ready(r),
            }
        }
    }

    fn is_terminal(&self) -> bool {
        use std::io::IsTerminal;
        self.file.is_terminal()
    }

    fn kind(&self) -> Option<FileKind> {
        self.file.metadata().ok().map(|m| metadata(&m).kind)
    }
}

/// The bound of a pump's buffer.
const PUMP_CAPACITY: usize = 64 * 1024;

#[derive(Default)]
struct PumpState {
    buf: VecDeque<u8>,
    eof: bool,
    error: bool,
    /// The engine dropped its end.
    closed: bool,
    waker: Option<Waker>,
}

struct PumpShared {
    state: Mutex<PumpState>,
    cond: Condvar,
}

impl PumpShared {
    fn wake(st: &mut PumpState) {
        if let Some(w) = st.waker.take() {
            w.wake();
        }
    }
}

/// The engine's end of a pipe to or from a child, served by a helper thread doing the
/// blocking reads or writes (v1 pumps, ARCH.md "psh-os").
struct PumpFile {
    shared: Arc<PumpShared>,
    reading: bool,
}

impl PumpFile {
    /// Reads `r` (a child's output) on a helper thread.
    fn reader(mut r: impl Read + Send + 'static) -> PumpFile {
        let shared = Arc::new(PumpShared {
            state: Mutex::new(PumpState::default()),
            cond: Condvar::new(),
        });
        let s = shared.clone();
        std::thread::spawn(move || {
            let mut tmp = vec![0u8; 16 * 1024];
            loop {
                {
                    let mut st = lock(&s.state);
                    while st.buf.len() >= PUMP_CAPACITY && !st.closed {
                        st = s.cond.wait(st).unwrap_or_else(|e| e.into_inner());
                    }
                    if st.closed {
                        return;
                    }
                }
                let r = r.read(&mut tmp);
                let mut st = lock(&s.state);
                match r {
                    Ok(0) => st.eof = true,
                    Ok(n) => st.buf.extend(&tmp[..n]),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => st.error = true,
                }
                let end = st.eof || st.error;
                PumpShared::wake(&mut st);
                if end || st.closed {
                    return;
                }
            }
        });
        PumpFile {
            shared,
            reading: true,
        }
    }

    /// Writes to `w` (a child's input) on a helper thread; dropping the engine's end
    /// flushes what is buffered and closes it.
    fn writer(mut w: impl Write + Send + 'static) -> PumpFile {
        let shared = Arc::new(PumpShared {
            state: Mutex::new(PumpState::default()),
            cond: Condvar::new(),
        });
        let s = shared.clone();
        std::thread::spawn(move || {
            loop {
                let chunk: Vec<u8> = {
                    let mut st = lock(&s.state);
                    while st.buf.is_empty() && !st.closed {
                        st = s.cond.wait(st).unwrap_or_else(|e| e.into_inner());
                    }
                    if st.buf.is_empty() {
                        return;
                    }
                    let n = st.buf.len().min(16 * 1024);
                    st.buf.drain(..n).collect()
                };
                let r = w.write_all(&chunk);
                let mut st = lock(&s.state);
                if r.is_err() {
                    st.error = true;
                    st.buf.clear();
                    PumpShared::wake(&mut st);
                    return;
                }
                PumpShared::wake(&mut st);
            }
        });
        PumpFile {
            shared,
            reading: false,
        }
    }
}

impl Drop for PumpFile {
    fn drop(&mut self) {
        let mut st = lock(&self.shared.state);
        st.closed = true;
        self.shared.cond.notify_all();
    }
}

impl HostFile for PumpFile {
    fn kind(&self) -> Option<FileKind> {
        Some(FileKind::Fifo)
    }

    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if !self.reading {
            return Poll::Ready(Err(crate::fd::bad_fd()));
        }
        let mut st = lock(&self.shared.state);
        if !st.buf.is_empty() {
            let n = buf.len().min(st.buf.len());
            for (i, b) in st.buf.drain(..n).enumerate() {
                buf[i] = b;
            }
            self.shared.cond.notify_all();
            return Poll::Ready(Ok(n));
        }
        if st.eof || st.error {
            return Poll::Ready(Ok(0));
        }
        st.waker = Some(cx.waker().clone());
        Poll::Pending
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.reading {
            return Poll::Ready(Err(crate::fd::bad_fd()));
        }
        let mut st = lock(&self.shared.state);
        if st.error {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let space = PUMP_CAPACITY.saturating_sub(st.buf.len());
        if space == 0 {
            st.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = space.min(buf.len());
        st.buf.extend(&buf[..n]);
        self.shared.cond.notify_all();
        Poll::Ready(Ok(n))
    }
}

#[derive(Default)]
struct ProcState {
    status: Option<ExitStatus>,
    waker: Option<Waker>,
    kill: bool,
    released: bool,
}

/// A child, waited for by its own thread.
struct StdProc {
    state: Arc<Mutex<ProcState>>,
}

fn exit_status(s: std::process::ExitStatus) -> ExitStatus {
    if let Some(c) = s.code() {
        return ExitStatus::Exited(c);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return ExitStatus::Signaled(sig);
        }
    }
    ExitStatus::Exited(1)
}

fn wait_thread(mut child: Child, state: Arc<Mutex<ProcState>>) {
    std::thread::spawn(move || {
        // std cannot wait and kill at once: poll with a short, growing sleep.
        let mut delay = Duration::from_micros(50);
        loop {
            let kill = lock(&state).kill;
            if kill {
                let _ = child.kill();
            }
            let st = match child.try_wait() {
                Ok(Some(s)) => exit_status(s),
                Ok(None) => {
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(Duration::from_millis(5));
                    continue;
                }
                Err(_) => ExitStatus::Exited(127),
            };
            let mut s = lock(&state);
            s.status = Some(st);
            if let Some(w) = s.waker.take() {
                w.wake();
            }
            return;
        }
    });
}

impl ExtProc for StdProc {
    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<ExitStatus> {
        let mut s = lock(&self.state);
        match s.status {
            Some(st) => Poll::Ready(st),
            None => {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    fn signal(&mut self, _sig: Signal) {
        lock(&self.state).kill = true;
    }

    fn release(self: Box<Self>) {
        // The waiter thread keeps waiting and reaps it.
        lock(&self.state).released = true;
    }
}

impl Drop for StdProc {
    fn drop(&mut self) {
        // Abandon, unless released: the waiter thread kills and reaps it.
        let mut s = lock(&self.state);
        if s.status.is_none() && !s.released {
            s.kill = true;
        }
    }
}

/// The host.
#[derive(Default)]
pub struct StdHost {
    pid: Option<u32>,
}

impl StdHost {
    pub fn new() -> StdHost {
        StdHost::default()
    }

    /// A host whose `$$` is `pid` rather than the process's own id: for an embedder
    /// that runs shells in-process, where `kill $$` must not reach the embedder.
    pub fn with_pid(pid: u32) -> StdHost {
        StdHost { pid: Some(pid) }
    }
}

fn ready<T: 'static>(v: T) -> HostFuture<T> {
    Box::pin(std::future::ready(v))
}

pub(crate) fn metadata(m: &std::fs::Metadata) -> Metadata {
    let ft = m.file_type();
    let mut kind = if ft.is_symlink() {
        FileKind::Symlink
    } else if ft.is_dir() {
        FileKind::Dir
    } else if ft.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    };
    let mtime_ns = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if ft.is_fifo() {
            kind = FileKind::Fifo;
        } else if ft.is_socket() {
            kind = FileKind::Socket;
        } else if ft.is_block_device() {
            kind = FileKind::BlockDevice;
        } else if ft.is_char_device() {
            kind = FileKind::CharDevice;
        }
        Metadata {
            kind,
            size: m.len(),
            mode: m.mode() & 0o7777,
            mtime_ns,
            dev: m.dev(),
            ino: m.ino(),
            uid: m.uid(),
            gid: m.gid(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = &mut kind;
        Metadata {
            kind,
            size: m.len(),
            mode: if m.permissions().readonly() {
                0o555
            } else {
                0o755
            },
            mtime_ns,
            dev: 0,
            ino: 0,
            uid: 0,
            gid: 0,
        }
    }
}

fn is_fifo(p: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(p).is_ok_and(|m| m.file_type().is_fifo())
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        false
    }
}

fn is_executable(p: &std::path::Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.is_file() && m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        m.is_file()
    }
}

/// Opens with `o`; on Unix, a file created with `opts.mode` gets exactly those bits
/// (exclusive create, then `fchmod`; an existing file keeps its own). Elsewhere the
/// mode is not applied.
fn open_with_mode(
    p: &std::path::Path,
    o: &mut std::fs::OpenOptions,
    opts: OpenOptions,
) -> io::Result<File> {
    #[cfg(unix)]
    if let Some(mode) = opts.mode.filter(|_| opts.create) {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        loop {
            let mut create = std::fs::OpenOptions::new();
            create
                .read(opts.read)
                .write(opts.write || opts.append)
                .append(opts.append)
                .create_new(true)
                .mode(mode);
            match create.open(p) {
                Ok(f) => {
                    f.set_permissions(std::fs::Permissions::from_mode(mode & 0o7777))?;
                    return Ok(f);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && !opts.create_new => {
                    let mut existing = std::fs::OpenOptions::new();
                    existing
                        .read(opts.read)
                        .write(opts.write && !opts.append)
                        .append(opts.append)
                        .truncate(opts.truncate);
                    match existing.open(p) {
                        Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                        r => return r,
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    let _ = opts;
    o.open(p)
}

/// The process's umask, read without changing it: Linux from `/proc/self/status`;
/// elsewhere the usual `0o022` (the std host has no way to read it safely).
fn process_umask() -> u32 {
    #[cfg(target_os = "linux")]
    if let Ok(status) = std::fs::read_to_string("/proc/self/status")
        && let Some(line) = status.lines().find(|l| l.starts_with("Umask:"))
        && let Ok(m) = u32::from_str_radix(line[6..].trim(), 8)
    {
        return m;
    }
    0o022
}

impl Host for StdHost {
    fn open(&self, p: &[u8], opts: OpenOptions) -> HostFuture<io::Result<Rc<dyn HostFile>>> {
        let mut o = std::fs::OpenOptions::new();
        o.read(opts.read)
            .write(opts.write && !opts.append)
            .append(opts.append)
            .truncate(opts.truncate)
            .create(opts.create && !opts.create_new)
            .create_new(opts.create_new);
        let p = if cfg!(windows) && p == b"/dev/null" {
            b"NUL".as_slice()
        } else {
            p
        };
        let p = path(p);
        if !is_fifo(&p) {
            return ready(
                open_with_mode(&p, &mut o, opts)
                    .map(|f| Rc::new(StdFile::new(f)) as Rc<dyn HostFile>),
            );
        }
        // Opening a FIFO waits for the other end, which may be a vproc of this
        // engine: open it on a helper thread (pitfall 1). If the open is abandoned,
        // the thread stays blocked until the FIFO's other end shows up.
        type Slot = (Option<io::Result<File>>, Option<Waker>);
        let state: Arc<Mutex<Slot>> = Arc::new(Mutex::new((None, None)));
        let st = state.clone();
        std::thread::spawn(move || {
            let r = o.open(&p);
            let mut s = lock(&st);
            s.0 = Some(r);
            if let Some(w) = s.1.take() {
                w.wake();
            }
        });
        Box::pin(std::future::poll_fn(move |cx| {
            let mut s = lock(&state);
            match s.0.take() {
                Some(r) => Poll::Ready(r.map(|f| Rc::new(StdFile::new(f)) as Rc<dyn HostFile>)),
                None => {
                    s.1 = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }))
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
        let r = std::fs::metadata(path(p)).map(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let bits = m.permissions().mode();
                match mode {
                    Access::Read => bits & 0o444 != 0,
                    Access::Write => bits & 0o222 != 0,
                    Access::Execute => bits & 0o111 != 0,
                }
            }
            #[cfg(not(unix))]
            {
                match mode {
                    Access::Write => !m.permissions().readonly(),
                    _ => true,
                }
            }
        });
        ready(r)
    }

    fn read_dir(&self, p: &[u8]) -> HostFuture<io::Result<Vec<DirEntry>>> {
        let r = std::fs::read_dir(path(p)).map(|it| {
            it.filter_map(Result::ok)
                .map(|e| DirEntry {
                    name: bytes(&e.file_name()),
                })
                .collect()
        });
        ready(r)
    }

    fn canonicalize(&self, p: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        ready(std::fs::canonicalize(path(p)).map(|p| bytes(p.as_os_str())))
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
        ready(m.map(|m| {
            let meta = metadata(&m);
            #[cfg(unix)]
            let extra = {
                use std::os::unix::fs::MetadataExt;
                let ns = |s: i64, n: i64| s.saturating_mul(1_000_000_000).saturating_add(n);
                ExtraMetadata {
                    atime_ns: ns(m.atime(), m.atime_nsec()),
                    ctime_ns: ns(m.ctime(), m.ctime_nsec()),
                    btime_ns: None,
                    nlink: m.nlink(),
                    blocks: m.blocks(),
                    blksize: m.blksize(),
                    rdev: m.rdev(),
                }
            };
            #[cfg(not(unix))]
            let extra = ExtraMetadata::derived(&meta);
            (meta, extra)
        }))
    }

    fn mkdir(&self, p: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        let r = std::fs::create_dir(path(p));
        #[cfg(unix)]
        let r = r.and_then(|()| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path(p), std::fs::Permissions::from_mode(mode & 0o7777))
        });
        #[cfg(not(unix))]
        let _ = mode;
        ready(r)
    }

    fn remove_file(&self, p: &[u8]) -> HostFuture<io::Result<()>> {
        ready(std::fs::remove_file(path(p)))
    }

    fn remove_dir(&self, p: &[u8]) -> HostFuture<io::Result<()>> {
        ready(std::fs::remove_dir(path(p)))
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> HostFuture<io::Result<()>> {
        ready(std::fs::rename(path(from), path(to)))
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        #[cfg(unix)]
        let r = std::os::unix::fs::symlink(path(target), path(link));
        #[cfg(not(unix))]
        let r = {
            let _ = (target, link);
            Err(io::ErrorKind::Unsupported.into())
        };
        ready(r)
    }

    fn hard_link(&self, existing: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        ready(std::fs::hard_link(path(existing), path(link)))
    }

    fn read_link(&self, p: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        ready(std::fs::read_link(path(p)).map(|t| bytes(t.as_os_str())))
    }

    fn set_mode(&self, p: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        let r = std::fs::metadata(path(p)).and_then(|m| {
            let mut perm = m.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                perm.set_mode(mode & 0o7777);
            }
            #[cfg(not(unix))]
            perm.set_readonly(mode & 0o222 == 0);
            std::fs::set_permissions(path(p), perm)
        });
        ready(r)
    }

    fn set_times(
        &self,
        p: &[u8],
        atime_ns: Option<i64>,
        mtime_ns: Option<i64>,
        follow: bool,
    ) -> HostFuture<io::Result<()>> {
        // std sets times through an open file, which follows symlinks.
        if !follow && std::fs::symlink_metadata(path(p)).is_ok_and(|m| m.is_symlink()) {
            return ready(Err(io::ErrorKind::Unsupported.into()));
        }
        let at = |ns: i64| {
            let d = Duration::from_nanos(ns.unsigned_abs());
            if ns >= 0 {
                std::time::UNIX_EPOCH + d
            } else {
                std::time::UNIX_EPOCH - d
            }
        };
        let mut times = std::fs::FileTimes::new();
        if let Some(a) = atime_ns {
            times = times.set_accessed(at(a));
        }
        if let Some(m) = mtime_ns {
            times = times.set_modified(at(m));
        }
        ready(File::open(path(p)).and_then(|f| f.set_times(times)))
    }

    fn resolve(&self, name: &[u8], path_var: &[u8], cwd: &[u8]) -> HostFuture<Option<Vec<u8>>> {
        let sep = if cfg!(windows) { b';' } else { b':' };
        let exts: Vec<String> = if cfg!(windows) {
            std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
                .split(';')
                .map(str::to_string)
                .chain(std::iter::once(String::new()))
                .collect()
        } else {
            vec![String::new()]
        };
        for dir in path_var.split(|&b| b == sep) {
            let mut d = if dir.is_empty() {
                cwd.to_vec()
            } else {
                dir.to_vec()
            };
            if d.first() != Some(&b'/') && !cfg!(windows) {
                let mut abs = cwd.to_vec();
                abs.push(b'/');
                abs.extend_from_slice(&d);
                d = abs;
            }
            for ext in &exts {
                let mut cand = d.clone();
                cand.push(b'/');
                cand.extend_from_slice(name);
                cand.extend_from_slice(ext.as_bytes());
                if is_executable(&path(&cand)) {
                    return ready(Some(cand));
                }
            }
        }
        ready(None)
    }

    fn spawn(&self, spec: SpawnSpec) -> io::Result<Spawned> {
        let mut cmd = Command::new(path(&spec.program));
        cmd.args(spec.argv.iter().skip(1).map(|a| os(a)))
            .env_clear()
            .envs(spec.env.iter().map(|(k, v)| (os(k), os(v))))
            .current_dir(path(&spec.cwd));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            if let Some(a0) = spec.argv.first() {
                cmd.arg0(os(a0));
            }
        }
        // The child's end of each descriptor, kept to serve `Dup`s.
        enum End {
            Null,
            File(File),
            Read(std::io::PipeReader),
            Write(std::io::PipeWriter),
        }
        fn stdio(end: &End) -> io::Result<Stdio> {
            Ok(match end {
                End::Null => Stdio::null(),
                End::File(f) => Stdio::from(f.try_clone()?),
                End::Read(r) => Stdio::from(r.try_clone()?),
                End::Write(w) => Stdio::from(w.try_clone()?),
            })
        }
        let mut ends: Vec<(u32, End)> = Vec::new();
        let mut pipes = Vec::new();
        for (n, fd) in spec.fds {
            if n > 2 {
                // Only the standard descriptors reach children in v1.
                continue;
            }
            let end = match fd {
                ChildFd::Null => End::Null,
                ChildFd::Pipe { input: true } => {
                    let (r, w) = std::io::pipe()?;
                    pipes.push((n, Rc::new(PumpFile::writer(w)) as Rc<dyn HostFile>));
                    End::Read(r)
                }
                ChildFd::Pipe { input: false } => {
                    let (r, w) = std::io::pipe()?;
                    pipes.push((n, Rc::new(PumpFile::reader(r)) as Rc<dyn HostFile>));
                    End::Write(w)
                }
                ChildFd::Dup(m) => {
                    let Some((_, e)) = ends.iter().find(|(k, _)| *k == m) else {
                        return Err(io::ErrorKind::InvalidInput.into());
                    };
                    match e {
                        End::Null => End::Null,
                        End::File(f) => End::File(f.try_clone()?),
                        End::Read(r) => End::Read(r.try_clone()?),
                        End::Write(w) => End::Write(w.try_clone()?),
                    }
                }
                ChildFd::File(f) => {
                    let any: &dyn std::any::Any = &*f;
                    match any.downcast_ref::<StdFile>() {
                        Some(sf) => End::File(sf.file.try_clone()?),
                        None => return Err(io::Error::other("not a file of this host")),
                    }
                }
            };
            let s = stdio(&end)?;
            match n {
                0 => cmd.stdin(s),
                1 => cmd.stdout(s),
                _ => cmd.stderr(s),
            };
            ends.push((n, end));
        }
        let mut tries = 0;
        let child = loop {
            match cmd.spawn() {
                Ok(c) => break c,
                // ETXTBSY: another thread's fork still holds a just-written script
                // open for writing until it execs. It passes in microseconds.
                Err(e) if e.raw_os_error() == Some(26) && cfg!(unix) && tries < 200 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        };
        // The child's ends must close here, or its readers never see EOF.
        drop(cmd);
        drop(ends);
        let state = Arc::new(Mutex::new(ProcState::default()));
        wait_thread(child, state.clone());
        Ok(Spawned {
            proc: Box::new(StdProc { state }),
            pipes,
        })
    }

    fn native_paths(&self) -> Option<crate::host::NativePathFn> {
        Some(|p| Ok(path(p)))
    }

    fn umask(&self) -> u32 {
        process_umask()
    }

    fn now(&self) -> Duration {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
    }

    fn sleep(&self, duration: Duration) -> HostFuture<()> {
        let state: Arc<Mutex<(bool, Option<Waker>)>> = Arc::new(Mutex::new((false, None)));
        let s = state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(duration);
            let mut st = lock(&s);
            st.0 = true;
            if let Some(w) = st.1.take() {
                w.wake();
            }
        });
        Box::pin(std::future::poll_fn(move |cx| {
            let mut st = lock(&state);
            if st.0 {
                Poll::Ready(())
            } else {
                st.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }))
    }

    fn pid(&self) -> u32 {
        self.pid.unwrap_or_else(std::process::id)
    }
}

/// The stack an engine thread should get: the default nesting limit at
/// [`crate::ShellConfig::STACK_PER_LEVEL`], plus headroom (D1050). Only touched pages
/// cost memory.
pub const ENGINE_STACK: usize = crate::ShellConfig::DEFAULT_MAX_DEPTH as usize
    * crate::ShellConfig::STACK_PER_LEVEL
    + (4 << 20);

/// Runs `f` on a new thread with [`ENGINE_STACK`].
pub fn engine_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> io::Result<std::thread::JoinHandle<T>> {
    std::thread::Builder::new()
        .name("psh-engine".into())
        .stack_size(ENGINE_STACK)
        .spawn(f)
}

/// Steps a shell on the current thread until it is done, parking while it is blocked.
pub fn drive(shell: &mut crate::Shell) -> i32 {
    drive_until(shell, None).unwrap_or(128 + crate::host::SIGTERM)
}

/// Like [`drive`], but cancels the shell at `deadline`: `None` means it timed out.
pub fn drive_until(shell: &mut crate::Shell, deadline: Option<std::time::Instant>) -> Option<i32> {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    shell.set_waker(Waker::from(Arc::new(Unpark(std::thread::current()))));
    let mut timed_out = false;
    loop {
        if !timed_out && deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            timed_out = true;
            shell.control().cancel();
        }
        match shell.step(10_000) {
            crate::Step::Done(s) => return if timed_out { None } else { Some(s) },
            crate::Step::Idle => return Some(0),
            crate::Step::Yield => {}
            // A wake before the park makes the park return at once (the token).
            crate::Step::Blocked => match deadline {
                Some(d) => std::thread::park_timeout(
                    d.saturating_duration_since(std::time::Instant::now()),
                ),
                None => std::thread::park(),
            },
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn open_with_mode_sets_exact_bits_on_create_only() {
        let dir = std::env::temp_dir().join(format!("psh-std-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f");
        let opts = OpenOptions {
            mode: Some(0o666),
            ..OpenOptions::write()
        };
        let mut o = std::fs::OpenOptions::new();
        open_with_mode(&p, &mut o, opts).unwrap();
        let mode = || std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(), 0o666);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        open_with_mode(&p, &mut o, opts).unwrap();
        assert_eq!(mode(), 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
        let h = StdHost::new();
        assert!(h.now() > Duration::from_secs(1_600_000_000));
    }
}
