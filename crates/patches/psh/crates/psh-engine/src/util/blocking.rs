//! Blocking utilities (D1080, D1130): code built on synchronous crates (the ripgrep
//! crates, jaq, a large `sort`) runs as `fn(&mut SyncCtx, T) -> i32` with std
//! `Read`/`Write` stdio and a synchronous file system view.
//!
//! Two backends, chosen by the Host:
//! - **Thread** (the Host names the process's own file system, [`Host::native_paths`]):
//!   the function runs on a worker thread; its stdio are queues the engine pumps from the
//!   utility's vproc, so pipes, `| head` and cancel behave as for any command. Its file
//!   system is `std::fs` on the OS paths. Output is bounded in flight
//!   ([`IN_FLIGHT`]): a writer waits for the engine, and a closed pipe fails its next
//!   write (EPIPE), so `rg x | head -1` stops early.
//! - **Inline** (wasm, sandbox and test hosts): the function runs on the engine thread.
//!   stdin, when the utility asks for it, is read to its end first, within
//!   [`INLINE_INPUT_LIMIT`]; output is collected in order and written after the function
//!   returns; file operations are Host futures polled once, so a host that cannot answer
//!   at once gets an error, never a hang.
//!
//! [`Host::native_paths`]: crate::host::Host::native_paths

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};

use super::{BUFFER, UtilCtx};
use crate::host::{DirEntry, Host, HostFile, Metadata, NativePathFn, OpenOptions};

/// Output a worker thread may have queued before it waits for the engine.
pub const IN_FLIGHT: usize = 1024 * 1024;

/// The most stdin an inline blocking utility reads before it runs (D1080).
pub const INLINE_INPUT_LIMIT: usize = 64 * 1024 * 1024;

/// The most output an inline blocking utility collects; writes past it fail
/// (`OutOfMemory`), so an endless writer ends instead of exhausting memory.
pub const INLINE_OUTPUT_LIMIT: usize = 256 * 1024 * 1024;

/// How a blocking utility runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockingOptions {
    /// It reads stdin. Only then does the inline backend read stdin (to its end, first);
    /// without it, stdin reads see end of file there.
    pub stdin: bool,
}

/// A blocking utility's body: its context and what the async front end prepared (the
/// parsed arguments, typically) to its exit status.
pub type SyncFn<T> = fn(&mut SyncCtx, T) -> i32;

/// What a blocking utility sees (D1130): the shell's cwd and environment as the command
/// sees them, std stdio, TTY state, a cancel flag and a synchronous file system. Paths
/// are the shell's (bytes, absolute after [`SyncCtx::path`]).
pub struct SyncCtx {
    name: String,
    cwd: Vec<u8>,
    env: Vec<(Vec<u8>, Vec<u8>)>,
    terminal: [bool; 3],
    stdin_readable: bool,
    io: Io,
    out: Vec<u8>,
    write_error: Option<io::ErrorKind>,
}

enum Io {
    Thread {
        pipe: Arc<Pipe>,
        native: NativePathFn,
    },
    Inline {
        host: Rc<dyn Host>,
        stdin: io::Cursor<Vec<u8>>,
        out: Arc<Mutex<InlineOut>>,
    },
}

/// The inline backend's output: what goes to fds 1 and 2, in order, and its size.
#[derive(Default)]
struct InlineOut {
    output: Vec<(u32, Vec<u8>)>,
    collected: usize,
}

impl InlineOut {
    fn push(&mut self, fd: u32, data: Vec<u8>) -> io::Result<()> {
        if self.collected + data.len() > INLINE_OUTPUT_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "output too large for an in-process utility in this environment",
            ));
        }
        self.collected += data.len();
        match self.output.last_mut() {
            Some((f, d)) if *f == fd => d.extend_from_slice(&data),
            _ => self.output.push((fd, data)),
        }
        Ok(())
    }
}

/// An unbuffered writer to the utility's fd 1 or 2 that may be cloned and moved to
/// other threads (rg's search threads print through it): writes go, in order with the
/// [`SyncCtx`]'s own, to the same queue. A failed write fails every later one, as for
/// [`SyncCtx::write`].
#[derive(Clone)]
pub struct SyncWriter {
    fd: u32,
    to: WriterTo,
}

#[derive(Clone)]
enum WriterTo {
    Pipe(Arc<Pipe>),
    Inline(Arc<Mutex<InlineOut>>),
}

impl Write for SyncWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match &self.to {
            WriterTo::Pipe(p) => p.write(self.fd, buf.to_vec())?,
            WriterTo::Inline(o) => lock_out(o).push(self.fd, buf.to_vec())?,
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn lock_out(m: &Mutex<InlineOut>) -> std::sync::MutexGuard<'_, InlineOut> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A file opened through [`SyncCtx::open`].
pub struct SyncFile {
    inner: FileInner,
}

enum FileInner {
    Os(std::fs::File),
    Host(Rc<dyn HostFile>),
}

impl Read for SyncFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.inner {
            FileInner::Os(f) => f.read(buf),
            FileInner::Host(f) => match poll_once(|cx| f.poll_read(cx, buf)) {
                Some(r) => r,
                None => Err(would_block()),
            },
        }
    }
}

fn would_block() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "the host cannot answer this at once",
    )
}

/// Polls once with a waker that does nothing: the inline backend's way of calling the
/// Host synchronously.
fn poll_once<T>(mut f: impl FnMut(&mut Context<'_>) -> Poll<T>) -> Option<T> {
    let mut cx = Context::from_waker(Waker::noop());
    match f(&mut cx) {
        Poll::Ready(v) => Some(v),
        Poll::Pending => None,
    }
}

fn block_on_once<T>(fut: crate::host::HostFuture<io::Result<T>>) -> io::Result<T> {
    let mut fut = fut;
    poll_once(|cx| fut.as_mut().poll(cx)).unwrap_or_else(|| Err(would_block()))
}

impl SyncCtx {
    /// The name the utility was called by, for messages.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The shell's working directory (logical, absolute).
    pub fn cwd(&self) -> &[u8] {
        &self.cwd
    }

    /// The environment the command sees, sorted by name.
    pub fn env(&self) -> &[(Vec<u8>, Vec<u8>)] {
        &self.env
    }

    pub fn var(&self, name: &[u8]) -> Option<&[u8]> {
        self.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_slice())
    }

    /// Whether descriptor `n` (0, 1 or 2) was a terminal when the utility started.
    pub fn is_terminal(&self, n: u32) -> bool {
        self.terminal.get(n as usize).copied().unwrap_or(false)
    }

    /// stdin is something to read: a pipe, a regular file or a socket, not a terminal,
    /// `/dev/null` or closed (rg's heuristic for searching stdin rather than `./`).
    pub fn stdin_readable(&self) -> bool {
        self.stdin_readable
    }

    /// A writer to fd 1 or 2 for other threads (stdout is flushed first).
    pub fn writer(&mut self, fd: u32) -> SyncWriter {
        let _ = self.flush();
        let to = match &self.io {
            Io::Thread { pipe, .. } => WriterTo::Pipe(pipe.clone()),
            Io::Inline { out, .. } => WriterTo::Inline(out.clone()),
        };
        SyncWriter { fd, to }
    }

    /// A path made absolute against the cwd (`.` and `..` kept, as the shell's cwd
    /// joins do).
    pub fn path(&self, p: &[u8]) -> Vec<u8> {
        if p.first() == Some(&b'/') || is_drive_path(p) {
            return p.to_vec();
        }
        let mut out = self.cwd.clone();
        if out.last() != Some(&b'/') {
            out.push(b'/');
        }
        out.extend_from_slice(p);
        out
    }

    /// The OS path of a shell path when the utility runs on the process's own file
    /// system (the thread backend): what `std::fs` and the `ignore` walker take.
    pub fn native_path(&self, p: &[u8]) -> Option<io::Result<PathBuf>> {
        match &self.io {
            Io::Thread { native, .. } => Some(native(&self.path(p))),
            Io::Inline { .. } => None,
        }
    }

    /// The stop was requested (the command was cancelled, or a signal ends the shell):
    /// long loops (walker visitors) check it and return.
    pub fn cancelled(&self) -> bool {
        match &self.io {
            Io::Thread { pipe, .. } => pipe.cancelled.load(Ordering::SeqCst),
            Io::Inline { .. } => false,
        }
    }

    /// Opens a file for reading.
    pub fn open(&self, p: &[u8]) -> io::Result<SyncFile> {
        let abs = self.path(p);
        match &self.io {
            Io::Thread { native, .. } => Ok(SyncFile {
                inner: FileInner::Os(std::fs::File::open(native(&abs)?)?),
            }),
            Io::Inline { host, .. } => {
                let opts = OpenOptions {
                    read: true,
                    ..OpenOptions::default()
                };
                Ok(SyncFile {
                    inner: FileInner::Host(block_on_once(host.open(&abs, opts))?),
                })
            }
        }
    }

    /// `stat` (`follow`) or `lstat`.
    pub fn stat(&self, p: &[u8], follow: bool) -> io::Result<Metadata> {
        let abs = self.path(p);
        match &self.io {
            Io::Thread { native, .. } => native_stat(&native(&abs)?, follow),
            Io::Inline { host, .. } => block_on_once(host.stat(&abs, follow)),
        }
    }

    /// The entries of a directory, without `.` and `..`, in no particular order.
    pub fn read_dir(&self, p: &[u8]) -> io::Result<Vec<DirEntry>> {
        let abs = self.path(p);
        match &self.io {
            Io::Thread { native, .. } => native_read_dir(&native(&abs)?),
            Io::Inline { host, .. } => block_on_once(host.read_dir(&abs)),
        }
    }

    /// Reads stdin (stdout is flushed first). `Ok(0)` is end of file.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let _ = self.flush();
        match &mut self.io {
            Io::Thread { pipe, .. } => pipe.read(buf),
            Io::Inline { stdin, .. } => stdin.read(buf),
        }
    }

    /// stdin as a `Read`.
    pub fn stdin(&mut self) -> Stdin<'_> {
        Stdin(self)
    }

    /// stdout as a `Write` (the context's buffer).
    pub fn stdout(&mut self) -> Stdout<'_> {
        Stdout(self)
    }

    /// stderr as a `Write` (stdout is flushed before each write).
    pub fn stderr(&mut self) -> Stderr<'_> {
        Stderr(self)
    }

    /// Writes to stdout through the buffer, flushed when full and at line ends on a
    /// terminal (D1081). After a failed write every later one fails the same way.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(kind) = self.write_error {
            return Err(kind.into());
        }
        self.out.extend_from_slice(data);
        if self.out.len() >= BUFFER || (self.terminal[1] && data.contains(&b'\n')) {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if self.out.is_empty() {
            return match self.write_error {
                Some(kind) => Err(kind.into()),
                None => Ok(()),
            };
        }
        let data = std::mem::take(&mut self.out);
        let r = self.send(1, data);
        if let Err(e) = &r {
            self.write_error = Some(e.kind());
        }
        r
    }

    /// Writes `NAME: msg` and a newline to stderr.
    pub fn error(&mut self, msg: &str) {
        let text = format!("{}: {msg}\n", self.name);
        self.error_raw(text.as_bytes());
    }

    /// Writes bytes to stderr (stdout is flushed first).
    pub fn error_raw(&mut self, data: &[u8]) {
        let _ = self.flush();
        let _ = self.send(2, data.to_vec());
    }

    fn send(&mut self, fd: u32, data: Vec<u8>) -> io::Result<()> {
        match &mut self.io {
            Io::Thread { pipe, .. } => pipe.write(fd, data),
            Io::Inline { out, .. } => lock_out(out).push(fd, data),
        }
    }
}

fn is_drive_path(p: &[u8]) -> bool {
    p.len() >= 2 && p[0].is_ascii_alphabetic() && p[1] == b':'
}

#[cfg(any(unix, windows))]
fn native_stat(p: &std::path::Path, follow: bool) -> io::Result<Metadata> {
    // Windows' null device (the host's `/dev/null`) has no metadata there; it is a
    // character device, as the host's own `stat` says.
    if cfg!(windows) && p.as_os_str().eq_ignore_ascii_case("NUL") {
        return Ok(Metadata {
            kind: crate::host::FileKind::CharDevice,
            size: 0,
            mode: 0o666,
            mtime_ns: 0,
            dev: 0,
            ino: 0,
            uid: 0,
            gid: 0,
        });
    }
    let m = if follow {
        std::fs::metadata(p)?
    } else {
        std::fs::symlink_metadata(p)?
    };
    Ok(crate::std_host::metadata(&m))
}

#[cfg(not(any(unix, windows)))]
fn native_stat(_: &std::path::Path, _: bool) -> io::Result<Metadata> {
    Err(io::ErrorKind::Unsupported.into())
}

fn native_read_dir(p: &std::path::Path) -> io::Result<Vec<DirEntry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(p)? {
        let name = e?.file_name();
        #[cfg(unix)]
        let name = {
            use std::os::unix::ffi::OsStrExt;
            name.as_bytes().to_vec()
        };
        #[cfg(not(unix))]
        let name = name.to_string_lossy().into_owned().into_bytes();
        out.push(DirEntry { name });
    }
    Ok(out)
}

/// [`SyncCtx::stdin`].
pub struct Stdin<'a>(&'a mut SyncCtx);

impl Read for Stdin<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

/// [`SyncCtx::stdout`].
pub struct Stdout<'a>(&'a mut SyncCtx);

impl Write for Stdout<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// [`SyncCtx::stderr`].
pub struct Stderr<'a>(&'a mut SyncCtx);

impl Write for Stderr<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.error_raw(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// --- the thread backend's queues ---------------------------------------------------

/// Between a worker thread and the engine task that pumps its stdio.
struct Pipe {
    state: Mutex<PipeState>,
    /// The thread waits on it for room, for stdin data and for the cancel.
    cv: Condvar,
    cancelled: AtomicBool,
}

#[derive(Default)]
struct PipeState {
    /// Output for fd 1 and 2, in order.
    out: VecDeque<(u32, Vec<u8>)>,
    queued: usize,
    /// A failed write to fd 1 or 2: later writes to it fail the same way.
    write_error: [Option<io::ErrorKind>; 3],
    /// The thread wants up to this many bytes of stdin.
    read_want: Option<usize>,
    /// The engine's answer: bytes (empty: end of file) or an error.
    read_answer: Option<io::Result<Vec<u8>>>,
    /// The thread ended (its status), or panicked (`None` with `ended`).
    status: Option<i32>,
    ended: bool,
    /// The engine task waiting for something to do.
    waker: Option<Waker>,
}

fn lock(m: &Mutex<PipeState>) -> std::sync::MutexGuard<'_, PipeState> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Pipe {
    fn wake(st: &mut PipeState) {
        if let Some(w) = st.waker.take() {
            w.wake();
        }
    }

    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Thread side: queues output, waiting while too much is in flight.
    fn write(&self, fd: u32, data: Vec<u8>) -> io::Result<()> {
        let mut st = lock(&self.state);
        loop {
            if self.cancelled() {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            if let Some(kind) = st.write_error[fd as usize] {
                return Err(kind.into());
            }
            if st.queued < IN_FLIGHT {
                break;
            }
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.queued += data.len();
        st.out.push_back((fd, data));
        Pipe::wake(&mut st);
        Ok(())
    }

    /// Thread side: one read of stdin through the engine.
    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut st = lock(&self.state);
        st.read_answer = None;
        st.read_want = Some(buf.len());
        Pipe::wake(&mut st);
        loop {
            if self.cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if let Some(ans) = st.read_answer.take() {
                let data = ans?;
                buf[..data.len()].copy_from_slice(&data);
                return Ok(data.len());
            }
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Thread side: the function returned (or panicked: `None`).
    fn finish(&self, status: Option<i32>) {
        let mut st = lock(&self.state);
        st.status = status;
        st.ended = true;
        Pipe::wake(&mut st);
    }
}

/// Engine side: dropping it (the command was cancelled) stops the thread's I/O, so a
/// thread left running can no longer write.
struct CancelOnDrop(Arc<Pipe>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        let _st = lock(&self.0.state);
        self.0.cv.notify_all();
    }
}

enum Event {
    Out(u32, Vec<u8>),
    Read(usize),
    Done(Option<i32>),
}

impl UtilCtx<'_> {
    /// Runs a blocking utility body (D1130): on a worker thread when the Host is the
    /// process's own file system, else inline. stdout is flushed first. The status is
    /// the function's; a panic in it is status 2 with a message.
    pub async fn run_blocking<T: Send + 'static>(
        &mut self,
        opts: BlockingOptions,
        arg: T,
        f: SyncFn<T>,
    ) -> i32 {
        let _ = self.flush().await;
        let terminal = [
            self.is_terminal(0),
            self.is_terminal(1),
            self.is_terminal(2),
        ];
        let host = self.vp.core.host.clone();
        let native = host.native_paths();
        let stdin_readable = self.stdin_readable();
        let mut sync = SyncCtx {
            name: self.name.clone(),
            cwd: self.vp.cwd.clone(),
            env: self.env.clone(),
            terminal,
            stdin_readable,
            io: Io::Inline {
                host,
                stdin: io::Cursor::new(Vec::new()),
                out: Arc::default(),
            },
            out: Vec::new(),
            write_error: None,
        };
        match native {
            Some(native) => self.run_thread(sync, native, arg, f).await,
            None => {
                if opts.stdin {
                    match self.read_limited(INLINE_INPUT_LIMIT).await {
                        Ok(data) => {
                            if let Io::Inline { stdin, .. } = &mut sync.io {
                                *stdin = io::Cursor::new(data);
                            }
                        }
                        Err(msg) => {
                            self.error(&msg).await;
                            return 2;
                        }
                    }
                }
                let status = run_guarded(&mut sync, arg, f);
                let _ = sync.flush();
                let Io::Inline { out, .. } = sync.io else {
                    unreachable!()
                };
                // Writers the body cloned are gone with its threads; take what they wrote.
                let output = std::mem::take(&mut lock_out(&out).output);
                for (fd, data) in output {
                    let r = match self.fd(fd) {
                        Some(file) => file.write_all(&data).await,
                        None => Err(crate::fd::bad_fd()),
                    };
                    if r.is_err() && fd == 1 {
                        break;
                    }
                }
                status
            }
        }
    }

    /// stdin to its end, or an error message past `limit` bytes.
    async fn read_limited(&mut self, limit: usize) -> Result<Vec<u8>, String> {
        let mut data = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match self.read(&mut buf).await {
                Ok(0) => return Ok(data),
                Ok(n) => {
                    if data.len() + n > limit {
                        return Err(format!(
                            "input too large for the in-process {} in this environment",
                            self.name
                        ));
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                Err(e) => return Err(format!("read error: {}", crate::host::error_text(&e))),
            }
        }
    }

    async fn run_thread<T: Send + 'static>(
        &mut self,
        sync: SyncCtx,
        native: NativePathFn,
        arg: T,
        f: SyncFn<T>,
    ) -> i32 {
        let pipe = Arc::new(Pipe {
            state: Mutex::new(PipeState::default()),
            cv: Condvar::new(),
            cancelled: AtomicBool::new(false),
        });
        let guard = CancelOnDrop(pipe.clone());
        // The parts that cross to the thread; the SyncCtx is rebuilt there (the inline
        // one holds the Host, which stays on the engine thread).
        let SyncCtx {
            name,
            cwd,
            env,
            terminal,
            stdin_readable,
            ..
        } = sync;
        let thread_pipe = pipe.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("psh-{name}"))
            .spawn(move || {
                let mut ctx = SyncCtx {
                    name,
                    cwd,
                    env,
                    terminal,
                    stdin_readable,
                    io: Io::Thread {
                        pipe: thread_pipe.clone(),
                        native,
                    },
                    out: Vec::new(),
                    write_error: None,
                };
                let status = run_guarded(&mut ctx, arg, f);
                let _ = ctx.flush();
                thread_pipe.finish(Some(status));
            });
        if let Err(e) = spawned {
            self.error(&format!("cannot start a thread: {e}")).await;
            return 2;
        }
        let mut buf = Vec::new();
        loop {
            let ev = std::future::poll_fn(|cx| {
                let mut st = lock(&pipe.state);
                if let Some((fd, data)) = st.out.pop_front() {
                    st.queued -= data.len();
                    pipe.cv.notify_all();
                    return Poll::Ready(Event::Out(fd, data));
                }
                if let Some(n) = st.read_want.take() {
                    return Poll::Ready(Event::Read(n));
                }
                if st.ended {
                    return Poll::Ready(Event::Done(st.status));
                }
                st.waker = Some(cx.waker().clone());
                Poll::Pending
            })
            .await;
            match ev {
                Event::Out(fd, data) => {
                    let r = match self.fd(fd) {
                        Some(file) => file.write_all(&data).await,
                        None => Err(crate::fd::bad_fd()),
                    };
                    if let Err(e) = r {
                        let mut st = lock(&pipe.state);
                        st.write_error[fd as usize] = Some(e.kind());
                        // What is still queued for that fd is lost with it.
                        let dropped: usize = st
                            .out
                            .iter()
                            .filter(|(f, _)| *f == fd)
                            .map(|(_, d)| d.len())
                            .sum();
                        st.out.retain(|(f, _)| *f != fd);
                        st.queued -= dropped;
                        pipe.cv.notify_all();
                    }
                }
                Event::Read(n) => {
                    buf.resize(n.min(BUFFER), 0);
                    let r = match self.fd(0) {
                        Some(file) => file.read(&mut buf).await.map(|k| buf[..k].to_vec()),
                        None => Err(crate::fd::bad_fd()),
                    };
                    let mut st = lock(&pipe.state);
                    st.read_answer = Some(r);
                    pipe.cv.notify_all();
                }
                Event::Done(status) => {
                    drop(guard);
                    return status.unwrap_or(2);
                }
            }
            crate::sched::safepoint(&self.vp.core.sched).await;
        }
    }
}

/// Runs `f`, turning a panic into status 2 and a message (a bug in ported code must not
/// take the shell down).
fn run_guarded<T>(ctx: &mut SyncCtx, arg: T, f: SyncFn<T>) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx, arg))) {
        Ok(status) => status,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_string());
            ctx.error(&format!("internal error: {msg}"));
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{LocalBoxFuture, Mode, UtilEntry, UtilPolicy};
    use crate::{Script, Shell, Step};

    /// `bupper`: stdin to stdout upper-cased, through `SyncCtx`'s `Read`/`Write`.
    fn bupper_body(ctx: &mut SyncCtx, _: ()) -> i32 {
        let mut data = Vec::new();
        if ctx.stdin().read_to_end(&mut data).is_err() {
            return 1;
        }
        ctx.error("note");
        match ctx.stdout().write_all(&data.to_ascii_uppercase()) {
            Ok(()) => 0,
            Err(_) => 141,
        }
    }

    fn bupper<'a>(ctx: &'a mut UtilCtx<'_>, _: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(ctx.run_blocking(BlockingOptions { stdin: true }, (), bupper_body))
    }

    /// `byes`: `y` lines until a write fails (status 0, as rg on a closed pipe), in
    /// 64 KiB writes.
    fn byes_body(ctx: &mut SyncCtx, _: ()) -> i32 {
        let chunk = b"y\n".repeat(32 * 1024);
        loop {
            if ctx.cancelled() || ctx.write(&chunk).is_err() {
                return 0;
            }
        }
    }

    fn byes<'a>(ctx: &'a mut UtilCtx<'_>, _: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(ctx.run_blocking(BlockingOptions::default(), (), byes_body))
    }

    /// `bcat FILE`: a file through the sync fs view, and `stat` of it.
    fn bcat_body(ctx: &mut SyncCtx, path: Vec<u8>) -> i32 {
        let size = match ctx.stat(&path, true) {
            Ok(m) => m.size,
            Err(_) => return 3,
        };
        let mut data = Vec::new();
        match ctx.open(&path).and_then(|mut f| f.read_to_end(&mut data)) {
            Ok(_) => {
                let _ = ctx.write(format!("{size}:").as_bytes());
                let _ = ctx.write(&data);
                0
            }
            Err(e) => {
                ctx.error(&crate::host::error_text(&e));
                1
            }
        }
    }

    fn bcat<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        let path = argv.get(1).cloned().unwrap_or_default();
        Box::pin(ctx.run_blocking(BlockingOptions::default(), path, bcat_body))
    }

    fn bpanic_body(_: &mut SyncCtx, _: ()) -> i32 {
        panic!("boom")
    }

    fn bpanic<'a>(ctx: &'a mut UtilCtx<'_>, _: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(ctx.run_blocking(BlockingOptions::default(), (), bpanic_body))
    }

    /// `bwriter`: `a` through the context, `b` and `e` (fd 2) through writers on another
    /// thread, then `c`: all in order.
    fn bwriter_body(ctx: &mut SyncCtx, _: ()) -> i32 {
        let _ = ctx.write(b"a\n");
        // Taking a writer flushes the context's own buffer first.
        let mut out = ctx.writer(1);
        let mut err = ctx.writer(2);
        std::thread::scope(|s| {
            s.spawn(move || {
                let _ = out.write_all(b"b\n");
                let _ = err.write_all(b"e\n");
            });
        });
        let _ = ctx.write(b"c\n");
        0
    }

    fn bwriter<'a>(ctx: &'a mut UtilCtx<'_>, _: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(ctx.run_blocking(BlockingOptions::default(), (), bwriter_body))
    }

    /// `breadable`: whether stdin would be read (`yes`/`no`).
    fn breadable_body(ctx: &mut SyncCtx, _: ()) -> i32 {
        let r = ctx.stdin_readable();
        let _ = ctx.write(if r { b"yes\n" } else { b"no\n" });
        0
    }

    fn breadable<'a>(ctx: &'a mut UtilCtx<'_>, _: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(ctx.run_blocking(BlockingOptions::default(), (), breadable_body))
    }

    const ENTRIES: &[UtilEntry] = &[
        UtilEntry {
            names: &["bwriter"],
            mode: Mode::Always,
            destructive: false,
            run: bwriter,
        },
        UtilEntry {
            names: &["breadable"],
            mode: Mode::Always,
            destructive: false,
            run: breadable,
        },
        UtilEntry {
            names: &["bupper"],
            mode: Mode::Always,
            destructive: false,
            run: bupper,
        },
        UtilEntry {
            names: &["byes"],
            mode: Mode::Always,
            destructive: false,
            run: byes,
        },
        UtilEntry {
            names: &["bcat"],
            mode: Mode::Always,
            destructive: false,
            run: bcat,
        },
        UtilEntry {
            names: &["bpanic"],
            mode: Mode::Always,
            destructive: false,
            run: bpanic,
        },
    ];

    fn policy() -> UtilPolicy {
        UtilPolicy {
            entries: ENTRIES,
            ..UtilPolicy::default()
        }
    }

    /// Inline: the test host has no native paths.
    fn inline(src: &str) -> (i32, String, String) {
        let host = Rc::new(crate::testhost::TestHost::new());
        host.set_stdin(b"abc\n");
        host.fs.write_file("/f.txt", b"file\n");
        let mut cfg = host.config();
        cfg.utilities = policy();
        let mut shell = Shell::new(cfg);
        shell.start(Script::command(src));
        let status = loop {
            match shell.step(7) {
                Step::Done(s) => break s,
                Step::Yield => {}
                other => panic!("{other:?} running {src:?}"),
            }
        };
        (
            status,
            String::from_utf8_lossy(&host.stdout()).into_owned(),
            String::from_utf8_lossy(&host.stderr()).into_owned(),
        )
    }

    #[test]
    fn inline_backend() {
        assert_eq!(
            inline("bupper"),
            (0, "ABC\n".into(), "bupper: note\n".into())
        );
        assert_eq!(inline("echo x | bupper | bupper").1, "X\n");
        assert_eq!(inline("bcat /f.txt").1, "5:file\n");
        assert_eq!(inline("cd /; bcat f.txt").1, "5:file\n");
        assert_eq!(inline("bcat /nope; echo $?").1, "3\n");
        // Inline output is written after the body: a reader that stops early still
        // ends the pipeline (byes stops at the first failed write... of the engine's).
        assert_eq!(inline("bpanic; echo $?").1, "2\n");
        // An endless writer ends at the output bound.
        assert_eq!(inline("byes | { read x; echo $x; }").1, "y\n");
        assert!(inline("bpanic").2.contains("bpanic: internal error: boom"));
        // Writers for other threads write in order with the context.
        assert_eq!(
            inline("bwriter 2>&1"),
            (0, "a\nb\ne\nc\n".into(), String::new())
        );
        assert_eq!(inline("bwriter 2>/dev/null").1, "a\nb\nc\n");
        // stdin: the test host's file is readable, an engine pipe is, /dev/null not.
        assert_eq!(inline("breadable").1, "yes\n");
        assert_eq!(inline("echo | breadable").1, "yes\n");
        assert_eq!(inline("breadable < /dev/null").1, "no\n");
    }

    /// Thread backend: the real file system, output to a file of a temp directory.
    #[cfg(unix)]
    fn threaded(dir: &std::path::Path, src: &str) -> i32 {
        let mut cfg = crate::ShellConfig::new(Rc::new(crate::std_host::StdHost::new()));
        cfg.utilities = policy();
        cfg.cwd = dir.to_string_lossy().into_owned().into_bytes();
        cfg.stdin = None;
        cfg.stdout = None;
        let mut shell = Shell::new(cfg);
        shell.start(Script::command(src));
        crate::std_host::drive_until(
            &mut shell,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(20)),
        )
        .expect("the script timed out")
    }

    #[cfg(unix)]
    #[test]
    fn thread_backend() {
        let dir = std::env::temp_dir().join(format!("psh-blocking-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        std::fs::write(dir.join("in"), "abc\ndef\n").unwrap();
        assert_eq!(threaded(&dir, "bupper < in > out 2> err"), 0);
        assert_eq!(
            (read("out"), read("err")),
            ("ABC\nDEF\n".into(), "bupper: note\n".into())
        );
        // Through engine pipes, both ways.
        assert_eq!(
            threaded(&dir, "printf 'q\\n' | bupper | bupper > out2 2>/dev/null"),
            0
        );
        assert_eq!(read("out2"), "Q\n");
        // A reader that stops closes the pipe: the thread's next write fails and the
        // pipeline ends instead of filling memory.
        assert_eq!(threaded(&dir, "byes | { read x; echo $x > out3; }"), 0);
        assert_eq!(read("out3"), "y\n");
        // The sync fs view: relative to the shell's cwd.
        assert_eq!(threaded(&dir, "bcat in > out4"), 0);
        assert_eq!(read("out4"), "8:abc\ndef\n");
        assert_eq!(threaded(&dir, "bpanic 2> err5; echo $? > out5"), 0);
        assert_eq!(read("out5"), "2\n");
        assert!(read("err5").contains("internal error: boom"));
        assert_eq!(threaded(&dir, "bwriter > out6 2>&1"), 0);
        assert_eq!(read("out6"), "a\nb\ne\nc\n");
        assert_eq!(
            threaded(
                &dir,
                "{ breadable < in; echo x | breadable; breadable < /dev/null; } > out7"
            ),
            0
        );
        assert_eq!(read("out7"), "yes\nyes\nno\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
