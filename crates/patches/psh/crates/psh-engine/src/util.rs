//! The utility API (D1080, D1081): in-process implementations of external programs
//! (`head`, `cat`, `timeout`, ... in psh-utils) run as async functions on the vproc that
//! runs the command, where shell builtins run. The embedder passes the registry in
//! [`crate::ShellConfig::utilities`]; the engine looks a name up after functions and
//! builtins and before `PATH`.
//!
//! Version 2 (`UTIL_API_VERSION`, D1081, D1150); a change to anything public here is a
//! new decision.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::fd::{Fd, FdObj, bad_fd};
use crate::host::{DirEntry, ExtraMetadata, Metadata, OpenOptions};
use crate::signals::Gives;
use crate::state::{R, Vp};

pub use crate::host::Access;

mod blocking;
mod nested;
pub use blocking::{
    BlockingOptions, IN_FLIGHT, INLINE_INPUT_LIMIT, INLINE_OUTPUT_LIMIT, Stderr, Stdin, Stdout,
    SyncCtx, SyncFile, SyncFiles, SyncFn, SyncWriter,
};

/// 2: file system changes, `stat_extra`, names, time and umask (D1150).
/// 3: `spawn_shell` (D1191).
pub const UTIL_API_VERSION: u32 = 3;

/// A boxed future that stays on the engine's thread.
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A utility: its arguments (argv[0] is the name it was called by) to its exit status.
pub type UtilFn = for<'a> fn(&'a mut UtilCtx<'_>, &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32>;

/// When a utility runs instead of a program of the same name (D015).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// S: always.
    Always,
    /// F: only if `PATH` has no such program (or the shell is a sandbox).
    PathFallback,
    /// W: always on Windows, as `PathFallback` elsewhere.
    WindowsShadow,
}

/// One utility of the registry.
#[derive(Clone, Copy)]
pub struct UtilEntry {
    /// The names it answers to (`timeout`, `gtimeout`).
    pub names: &'static [&'static str],
    pub mode: Mode,
    /// In the destructive group (`rm mv cp ln dd`), which can be turned off as one.
    pub destructive: bool,
    pub run: UtilFn,
}

/// The utilities of a shell and which are turned off.
#[derive(Clone, Default)]
pub struct UtilPolicy {
    pub entries: &'static [UtilEntry],
    /// No utility runs.
    pub off_all: bool,
    /// No utility of the destructive group runs.
    pub off_destructive: bool,
    /// These names run the real program (or are not found).
    pub off_names: Vec<Vec<u8>>,
    /// These names run the utility even in `PathFallback` and `WindowsShadow` mode,
    /// when `PATH` has the program (D1192: the L4 cases test psh's utilities).
    pub force_names: Vec<Vec<u8>>,
    /// There are no real programs (wasm, a sandbox host): `PathFallback` and
    /// `WindowsShadow` utilities always run.
    pub sandbox: bool,
}

/// The utility that runs for `name`, if any: off switches and modes applied.
pub(crate) async fn lookup(vp: &Vp, name: &[u8]) -> Option<&'static UtilEntry> {
    // A path, or a name bound to a path with `hash -p` (or remembered from running the
    // program), names the program; `bash` and `sh` run in psh before utilities (D1410).
    if crate::builtins::hash::hashed(vp, name).is_some() {
        return None;
    }
    if nested::is_shell(vp, name) {
        return Some(&nested::ENTRY);
    }
    if name.contains(&b'/') {
        return None;
    }
    let policy = &vp.core.utilities;
    if policy.off_all || policy.off_names.iter().any(|n| n == name) {
        return None;
    }
    let entries: &'static [UtilEntry] = policy.entries;
    let entry = entries
        .iter()
        .find(|e| e.names.iter().any(|n| n.as_bytes() == name))?;
    if entry.destructive && policy.off_destructive {
        return None;
    }
    let fallback = match entry.mode {
        Mode::Always => false,
        Mode::PathFallback => true,
        Mode::WindowsShadow => !cfg!(windows),
    };
    if fallback
        && !policy.sandbox
        && !policy.force_names.iter().any(|n| n == name)
        && crate::builtins::hash::search(vp, name, None)
            .await
            .is_some()
    {
        return None;
    }
    Some(entry)
}

pub(crate) use nested::start as start_nested;

/// Whether `entry` is the shell psh runs for `bash` and `sh` (D1410).
pub(crate) fn is_nested_shell(entry: &UtilEntry) -> bool {
    std::ptr::eq(entry, &nested::ENTRY)
}

/// Runs a utility with the prefix assignments of its command line in its environment.
pub(crate) async fn run(
    vp: &mut Vp,
    entry: &'static UtilEntry,
    argv: Vec<Vec<u8>>,
    assigns: Vec<(String, Vec<u8>)>,
) -> R {
    run_with(vp, entry, argv, assigns, false).await
}

/// Runs a utility for `exec`, in place of the shell: a `bash` run so keeps the
/// caller's `$$` and `SHLVL`, as the process it replaces would (D1410).
pub(crate) async fn run_exec(vp: &mut Vp, entry: &'static UtilEntry, argv: Vec<Vec<u8>>) -> R {
    run_with(vp, entry, argv, Vec::new(), true).await
}

async fn run_with(
    vp: &mut Vp,
    entry: &'static UtilEntry,
    argv: Vec<Vec<u8>>,
    assigns: Vec<(String, Vec<u8>)>,
    in_place: bool,
) -> R {
    // A virtual process, started where externals start (D1411).
    if is_nested_shell(entry) {
        let opts = crate::exec::ExtOpts {
            shell: true,
            exec: in_place,
            ..Default::default()
        };
        return crate::exec::run_external_with(vp, argv, assigns, opts).await;
    }
    let env = vp.env(&assigns);
    let name = String::from_utf8_lossy(&argv[0]).into_owned();
    let replay = Rc::new(std::cell::RefCell::new(blocking::Replay::default()));
    let mut ctx = UtilCtx {
        vp,
        name,
        env,
        out: Vec::new(),
        write_error: None,
        zone: None,
        replay: replay.clone(),
    };
    // A signal that ends the shell ends the utility, as it ends the wait for an
    // external (D1903). The vproc is the utility's now: watch with what it needs.
    let watch = (ctx.vp.top && ctx.vp.in_trap.is_none()).then(|| {
        (
            ctx.vp.core.clone(),
            ctx.vp.sigs.clone(),
            ctx.vp.traps.clone(),
        )
    });
    let status = loop {
        let ran = {
            let mut fut = std::pin::pin!((entry.run)(&mut ctx, &argv));
            std::future::poll_fn(|cx| {
                if let Some((core, sigs, traps)) = &watch
                    && let Some(sig) =
                        crate::signals::breaking_in(core, sigs, traps, Gives::Interrupt)
                {
                    return Poll::Ready(Err(sig));
                }
                match fut.as_mut().poll(cx) {
                    Poll::Ready(st) => Poll::Ready(Ok(Some(st))),
                    // A blocking body asked a host that answers later: this run is
                    // dropped and the utility starts again on the answers.
                    Poll::Pending if replay.borrow().again => Poll::Ready(Ok(None)),
                    Poll::Pending => {
                        if let Some((core, sigs, _)) = &watch {
                            sigs.on_signal(core, cx.waker());
                        }
                        Poll::Pending
                    }
                }
            })
            .await
        };
        match ran {
            Ok(Some(st)) => break Ok(st),
            Ok(None) => {
                blocking::Replay::settle(&replay).await;
                ctx.out.clear();
                ctx.write_error = None;
            }
            Err(sig) => break Err(sig),
        }
    };
    match status {
        Ok(status) => {
            // What is still buffered goes out now; a utility that cares about a
            // failing final write flushes itself before returning.
            let _ = ctx.flush().await;
            Ok(status)
        }
        // The shell is ending, or the signal ended the utility as it would have ended
        // a program: what it had buffered is dropped with it.
        Err(sig) => Ok(128 + sig),
    }
}

/// stdout is flushed when the buffer holds this much (D1081).
const BUFFER: usize = 64 * 1024;

/// What a utility sees of the shell (D1081): its descriptors, cwd, environment and the
/// Host. Created by the engine for each run.
pub struct UtilCtx<'v> {
    vp: &'v mut Vp,
    name: String,
    env: Vec<(Vec<u8>, Vec<u8>)>,
    out: Vec<u8>,
    /// A failed stdout write (a flush before a read, say): every later write fails with
    /// it, so a utility writing into a closed pipe stops (EPIPE).
    write_error: Option<io::ErrorKind>,
    /// The time zone, once [`UtilCtx::load_zone`] ran (`Some(None)`: none readable).
    zone: Option<Option<crate::builtins::strftime::Zone>>,
    /// What a blocking body asked of a host that answers later (`blocking::Replay`).
    replay: Rc<std::cell::RefCell<blocking::Replay>>,
}

/// How [`UtilCtx::spawn`] changes the environment of the command it runs.
#[derive(Clone, Debug, Default)]
pub struct SpawnEnv {
    /// Start from an empty environment (`env -i`).
    pub clear: bool,
    /// Variables to remove (`env -u NAME`).
    pub unset: Vec<String>,
    /// Variables to set, in order (`env NAME=value`).
    pub set: Vec<(String, Vec<u8>)>,
    /// The command's working directory instead of the utility's (`find -execdir`, `env -C`), an
    /// absolute shell path.
    pub cwd: Option<Vec<u8>>,
    /// A file opened as the command's stdin instead of the utility's (`xargs` gives its
    /// commands `/dev/null`, or `/dev/tty` with `-o`); closed if it cannot be opened.
    pub stdin: Option<Vec<u8>>,
}

/// Shell builtins that also exist as programs, so wrappers (`env`, `timeout`, `xargs`)
/// run them (D015). Other builtins are not found from a wrapper, as in bash.
pub const STANDALONE_BUILTINS: &[&str] = &[
    "printf", "echo", "test", "[", "true", "false", "pwd", "kill",
];

/// A command started by [`UtilCtx::spawn`]; dropping it stops the command (its externals
/// are abandoned: TERM, then KILL).
pub struct UtilChild {
    fut: LocalBoxFuture<'static, i32>,
    status: Option<i32>,
}

impl UtilChild {
    /// Its exit status, once it ended.
    pub fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<i32> {
        if let Some(s) = self.status {
            return Poll::Ready(s);
        }
        match self.fut.as_mut().poll(cx) {
            Poll::Ready(s) => {
                self.status = Some(s);
                Poll::Ready(s)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    pub async fn wait(&mut self) -> i32 {
        std::future::poll_fn(|cx| self.poll_wait(cx)).await
    }
}

/// Where a stream of [`UtilCtx::spawn_shell`]'s shell goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellIo {
    /// The utility's own descriptor.
    Inherit,
    /// A pipe whose other end is the utility's ([`ShellChild::stdin`], `stdout`).
    Pipe,
}

/// The stdin and stdout of a [`UtilCtx::spawn_shell`] shell; stderr and every other
/// descriptor are the utility's.
#[derive(Clone, Copy, Debug)]
pub struct ShellStdio {
    pub stdin: ShellIo,
    pub stdout: ShellIo,
}

/// A shell started by [`UtilCtx::spawn_shell`]. It runs as a task of its own, so the
/// utility can write its stdin and read its stdout meanwhile. Dropping it before it
/// ended stops it (its externals are abandoned: TERM, then KILL).
pub struct ShellChild {
    /// The write end of its stdin pipe (`ShellIo::Pipe`); drop it to close it.
    pub stdin: Option<UtilFile>,
    /// The read end of its stdout pipe (`ShellIo::Pipe`); drop it to close it.
    pub stdout: Option<UtilFile>,
    task: Option<(Rc<crate::sched::Sched>, usize)>,
    join: Option<crate::sched::Join<i32>>,
    status: Option<i32>,
}

impl ShellChild {
    /// Its exit status, once it ended. Closes its stdin first, as it would otherwise
    /// wait for more input; read its stdout to the end (or drop it) before waiting, or a
    /// shell that fills the pipe never ends.
    pub async fn wait(&mut self) -> i32 {
        self.stdin = None;
        if let Some(s) = self.status {
            return s;
        }
        let s = match &mut self.join {
            Some(j) => j.await,
            None => 2,
        };
        self.status = Some(s);
        self.join = None;
        s
    }
}

impl Drop for ShellChild {
    fn drop(&mut self) {
        if let (Some((sched, id)), Some(join)) = (&self.task, &self.join)
            && self.status.is_none()
            && !join.is_done()
        {
            sched.drop_task(*id);
        }
    }
}

/// An open file: one of the Host, or one of the shell's descriptors (`/dev/fd/N`,
/// `/dev/stdin`, as the engine emulates them, D1055).
pub struct UtilFile {
    file: Fd,
}

impl UtilFile {
    /// What the file is, where its host can tell (`HostFile::kind`); one of the shell's
    /// pipes is a FIFO.
    pub fn kind(&self) -> Option<crate::host::FileKind> {
        match &*self.file {
            FdObj::File(file) => file.kind(),
            FdObj::PipeRead(_) | FdObj::PipeWrite(_) => Some(crate::host::FileKind::Fifo),
        }
    }

    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf).await
    }

    pub async fn read_to_end(&self, out: &mut Vec<u8>) -> io::Result<()> {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = self.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    pub async fn write_all(&self, data: &[u8]) -> io::Result<()> {
        self.file.write_all(data).await
    }
}

impl UtilCtx<'_> {
    /// What the host says of itself and of the user (`uname`, `id`; nanocode's
    /// addition).
    pub fn host_info(&self) -> crate::host::HostInfo {
        self.vp.host().info()
    }

    /// The name the utility was called by (`argv[0]`), for messages.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The shell's working directory (logical, absolute).
    pub fn cwd(&self) -> &[u8] {
        &self.vp.cwd
    }

    /// The environment the command sees, sorted by name.
    pub fn env(&self) -> &[(Vec<u8>, Vec<u8>)] {
        &self.env
    }

    /// One environment variable.
    pub fn var(&self, name: &[u8]) -> Option<&[u8]> {
        self.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_slice())
    }

    /// The locale is UTF-8 (`LC_ALL`, `LC_CTYPE`, `LANG`).
    pub fn utf8(&self) -> bool {
        let get = |n: &[u8]| self.var(n).filter(|v| !v.is_empty());
        let v = get(b"LC_ALL")
            .or_else(|| get(b"LC_CTYPE"))
            .or_else(|| get(b"LANG"));
        v.is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.windows(5).any(|w| w == b"utf-8") || v.windows(4).any(|w| w == b"utf8")
        })
    }

    /// A path made absolute against the cwd.
    pub fn path(&self, p: &[u8]) -> Vec<u8> {
        self.vp.abs_path(p)
    }

    fn fd(&self, n: u32) -> Option<Fd> {
        self.vp.fd(n)
    }

    /// Whether descriptor `n` is a terminal.
    pub fn is_terminal(&self, n: u32) -> bool {
        matches!(self.fd(n).as_deref(), Some(FdObj::File(f)) if f.is_terminal())
    }

    /// stdin is something to read: one of the shell's pipes, or a host file that is a
    /// regular file, a pipe or a socket; not a terminal, a device such as `/dev/null`,
    /// closed, or of a kind the host cannot tell.
    pub fn stdin_readable(&self) -> bool {
        use crate::host::FileKind;
        match self.fd(0).as_deref() {
            Some(FdObj::PipeRead(_)) => true,
            Some(FdObj::File(f)) => {
                !f.is_terminal()
                    && matches!(
                        f.kind(),
                        Some(FileKind::File | FileKind::Fifo | FileKind::Socket)
                    )
            }
            _ => false,
        }
    }

    /// Reads stdin (stdout is flushed first). `Ok(0)` is end of file.
    pub async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let _ = self.flush().await;
        match self.fd(0) {
            Some(f) => f.read(buf).await,
            None => Err(bad_fd()),
        }
    }

    /// Reads stdin to its end.
    pub async fn read_to_end(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        let _ = self.flush().await;
        match self.fd(0) {
            Some(f) => f.read_to_end(out).await,
            None => Err(bad_fd()),
        }
    }

    /// Writes to stdout through the buffer (flushed when full, and at line ends when
    /// stdout is a terminal).
    pub async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(kind) = self.write_error {
            return Err(kind.into());
        }
        self.out.extend_from_slice(data);
        if self.out.len() >= BUFFER || (data.contains(&b'\n') && self.is_terminal(1)) {
            self.flush().await?;
        }
        Ok(())
    }

    /// Writes the buffer to stdout. The buffer is emptied even when the write fails.
    pub async fn flush(&mut self) -> io::Result<()> {
        if self.out.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.out);
        let r = match self.fd(1) {
            Some(f) => f.write_all(&data).await,
            None => Err(bad_fd()),
        };
        if let Err(e) = &r {
            self.write_error = Some(e.kind());
        }
        r
    }

    /// Writes `NAME: msg` and a newline to stderr (stdout is flushed first).
    pub async fn error(&mut self, msg: &str) {
        let text = format!("{}: {msg}\n", self.name);
        self.error_raw(text.as_bytes()).await;
    }

    /// Writes bytes to stderr as they are (stdout is flushed first).
    pub async fn error_raw(&mut self, data: &[u8]) {
        let _ = self.flush().await;
        if let Some(f) = self.fd(2) {
            let _ = f.write_all(data).await;
        }
    }

    /// Opens a file (relative paths against the cwd).
    pub async fn open(&self, path: &[u8], opts: OpenOptions) -> io::Result<UtilFile> {
        // The shell's own descriptors, as for redirections (D1055): process
        // substitutions are engine pipes no host knows.
        if let Some(n) = crate::exec::dev_fd(path) {
            return self.fd(n).map(|file| UtilFile { file }).ok_or_else(bad_fd);
        }
        // Files a utility creates get the shell's umask, as a child process would.
        let opts = crate::builtins::umask::creation_mode(self.vp, opts);
        let file = self.vp.host().open(&self.path(path), opts).await?;
        Ok(UtilFile {
            file: Rc::new(FdObj::File(file)),
        })
    }

    /// `stat` (`follow`) or `lstat`.
    pub async fn stat(&self, path: &[u8], follow: bool) -> io::Result<Metadata> {
        self.vp.host().stat(&self.path(path), follow).await
    }

    pub async fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
        self.vp.host().read_dir(&self.path(path)).await
    }

    /// The entries of a directory with the kinds its listing tells
    /// ([`crate::host::Host::read_dir_kinds`]).
    pub async fn read_dir_kinds(&self, path: &[u8]) -> io::Result<Vec<crate::host::KnownEntry>> {
        self.vp.host().read_dir_kinds(&self.path(path)).await
    }

    pub async fn access(&self, path: &[u8], mode: Access) -> io::Result<bool> {
        self.vp.host().access(&self.path(path), mode).await
    }

    pub async fn canonicalize(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        self.vp.host().canonicalize(&self.path(path)).await
    }

    // Version 2 (D1150): the rest of stat, file system changes, names, time.

    /// `stat` plus the rest of `stat(2)` (links, blocks, other times).
    pub async fn stat_extra(
        &self,
        path: &[u8],
        follow: bool,
    ) -> io::Result<(Metadata, ExtraMetadata)> {
        self.vp.host().stat_extra(&self.path(path), follow).await
    }

    /// Creates one directory with exactly these permission bits (apply [`Self::umask`]
    /// first for the default).
    pub async fn mkdir(&self, path: &[u8], mode: u32) -> io::Result<()> {
        self.vp.host().mkdir(&self.path(path), mode).await
    }

    /// Removes a name that is not a directory (a symlink itself, never its target).
    pub async fn remove_file(&self, path: &[u8]) -> io::Result<()> {
        self.vp.host().remove_file(&self.path(path)).await
    }

    /// Removes an empty directory.
    pub async fn remove_dir(&self, path: &[u8]) -> io::Result<()> {
        self.vp.host().remove_dir(&self.path(path)).await
    }

    pub async fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        let (from, to) = (self.path(from), self.path(to));
        self.vp.host().rename(&from, &to).await
    }

    /// Creates the symlink `link` to `target` (stored as given, not made absolute).
    pub async fn symlink(&self, target: &[u8], link: &[u8]) -> io::Result<()> {
        self.vp.host().symlink(target, &self.path(link)).await
    }

    pub async fn hard_link(&self, existing: &[u8], link: &[u8]) -> io::Result<()> {
        let (existing, link) = (self.path(existing), self.path(link));
        self.vp.host().hard_link(&existing, &link).await
    }

    /// A symlink's target as stored.
    pub async fn read_link(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        self.vp.host().read_link(&self.path(path)).await
    }

    /// `chmod` (follows symlinks).
    pub async fn set_mode(&self, path: &[u8], mode: u32) -> io::Result<()> {
        self.vp.host().set_mode(&self.path(path), mode).await
    }

    /// Sets the access and modification times (ns since the epoch; `None` keeps one).
    pub async fn set_times(
        &self,
        path: &[u8],
        atime_ns: Option<i64>,
        mtime_ns: Option<i64>,
        follow: bool,
    ) -> io::Result<()> {
        self.vp
            .host()
            .set_times(&self.path(path), atime_ns, mtime_ns, follow)
            .await
    }

    pub fn user_name(&self, uid: u32) -> Option<Vec<u8>> {
        self.vp.host().user_name(uid)
    }

    pub fn group_name(&self, gid: u32) -> Option<Vec<u8>> {
        self.vp.host().group_name(gid)
    }

    /// Loads the command's time zone (its `TZ`: a zone file, a path or a POSIX rule;
    /// else `/etc/localtime`), so that [`utc_offset`](Self::utc_offset) follows it and
    /// [`zone_abbr`](Self::zone_abbr) names it. Without zone files (wasm, Windows) the
    /// host's offset stays in use.
    pub async fn load_zone(&mut self) {
        if self.zone.is_none() {
            let tz = self.var(b"TZ").map(<[u8]>::to_vec);
            let z = crate::builtins::strftime::load(self.vp.host(), tz.as_deref()).await;
            self.zone = Some(z);
        }
    }

    /// Uses UTC from now on, whatever `TZ` says (`date -u`).
    pub fn use_utc(&mut self) {
        self.zone = Some(Some(crate::builtins::strftime::Zone::utc()));
    }

    /// The zone's abbreviation at `epoch_secs` (`UTC`, `CEST`), once [`load_zone`]
    /// found one; else `UTC` for a zero offset and the numeric offset (`+0100`).
    ///
    /// [`load_zone`]: Self::load_zone
    pub fn zone_abbr(&self, epoch_secs: i64) -> String {
        if let Some(Some(z)) = &self.zone {
            return String::from_utf8_lossy(&z.local_at(epoch_secs).1).into_owned();
        }
        let off = self.utc_offset(epoch_secs);
        if off == 0 {
            return "UTC".into();
        }
        let a = off.abs();
        let sign = if off < 0 { '-' } else { '+' };
        format!("{sign}{:02}{:02}", a / 3600, a / 60 % 60)
    }

    /// The local time's offset east of UTC at `epoch_secs`, in seconds: the loaded zone
    /// ([`load_zone`](Self::load_zone)), else the host's time zone, or UTC when the
    /// command's `TZ` names UTC (`UTC`, `UTC0`, `GMT`, `Etc/UTC`, empty).
    pub fn utc_offset(&self, epoch_secs: i64) -> i32 {
        if let Some(Some(z)) = &self.zone {
            return z.local_at(epoch_secs).0 as i32;
        }
        if let Some(tz) = self.var(b"TZ") {
            let tz = tz.strip_prefix(b":").unwrap_or(tz);
            if matches!(
                tz,
                b"" | b"UTC" | b"UTC0" | b"GMT" | b"GMT0" | b"Etc/UTC" | b"Etc/GMT" | b"UCT"
            ) {
                return 0;
            }
        }
        self.vp.host().utc_offset(epoch_secs)
    }

    /// The current time since the epoch.
    pub fn now(&self) -> Duration {
        self.vp.host().now()
    }

    /// The shell's file creation mask (`umask`).
    pub fn umask(&self) -> u32 {
        self.vp.umask.unwrap_or_else(|| self.vp.host().umask())
    }

    /// Completes after `duration` (cancellable: dropping it stops the wait).
    pub async fn sleep(&self, duration: Duration) {
        self.vp.host().sleep(duration).await;
    }

    /// A safepoint for long loops: yields when the step's budget is spent.
    pub async fn check(&self) {
        crate::sched::checkpoint(&self.vp.core.sched).await;
    }

    /// Whether `name` runs as one of psh's utilities here: what `which` reports.
    pub async fn is_utility(&self, name: &[u8]) -> bool {
        lookup(self.vp, name).await.is_some()
    }

    /// Every program named `name` (no slash) in the directories of `path`, in order:
    /// what `which -a` lists. A relative directory gives a relative path, as in
    /// `type`.
    pub async fn search_path(&self, name: &[u8], path: &[u8]) -> Vec<Vec<u8>> {
        let host = self.vp.host();
        let mut found = Vec::new();
        for dir in host.path_entries(path) {
            let Some(hit) = host.resolve(name, &dir, &self.vp.cwd).await else {
                continue;
            };
            if dir.is_empty() || self.vp.abs_path(&dir) == dir {
                found.push(hit);
            } else {
                // The host may have added an extension (PATHEXT): keep its file name.
                let base = hit.rsplit(|&b| b == b'/').next().unwrap_or(name);
                let mut shown = dir.clone();
                shown.push(b'/');
                shown.extend_from_slice(base);
                found.push(shown);
            }
        }
        found
    }

    /// Runs the real program of this name from `PATH` with these arguments, for an
    /// option the utility does not implement (D1082): its status, or `None` when there
    /// is no such program (or the shell is a sandbox, where none exist).
    pub async fn run_program(&mut self, argv: Vec<Vec<u8>>) -> Option<i32> {
        if self.vp.core.utilities.sandbox || argv.is_empty() {
            return None;
        }
        crate::builtins::hash::search(self.vp, &argv[0], None).await?;
        let _ = self.flush().await;
        let mut sub = self.vp.subshell();
        for (k, v) in &self.env {
            if let Ok(name) = std::str::from_utf8(k)
                && crate::state::is_name(k)
            {
                let var = sub.vars.entry(name);
                var.set(v.clone());
                var.exported = true;
            }
        }
        let r = crate::exec::run_external(&mut sub, argv, Vec::new()).await;
        let last = sub.status;
        Some(r.unwrap_or_else(|u| u.status(last)))
    }

    /// Starts a command for a wrapper (`env`, `timeout`, `xargs`, `find -exec`): a
    /// standalone builtin ([`STANDALONE_BUILTINS`]), a utility or an external, with this
    /// context's descriptors and cwd and the environment changed by `env`. stdout is
    /// flushed first. The command runs when the returned child is awaited.
    pub async fn spawn(&mut self, argv: Vec<Vec<u8>>, env: SpawnEnv) -> UtilChild {
        let _ = self.flush().await;
        let mut sub = self.vp.subshell();
        // The command sees the environment of the utility (prefix assignments
        // included), changed by `env`.
        if env.clear {
            let names: Vec<String> = sub.vars.iter().map(|(n, _)| n.clone()).collect();
            for n in names {
                if let Some(var) = sub.vars.get_mut(&n) {
                    var.exported = false;
                }
            }
            sub.foreign_env = Rc::new(Vec::new());
        } else {
            for (k, v) in &self.env {
                if let Ok(name) = std::str::from_utf8(k)
                    && crate::state::is_name(k)
                {
                    let var = sub.vars.entry(name);
                    var.set(v.clone());
                    var.exported = true;
                }
            }
        }
        for name in &env.unset {
            sub.vars.remove(name);
        }
        // Set as variables of the child, not as prefix assignments: the command is
        // looked up with them (`env PATH=/x cmd` searches /x).
        for (name, value) in env.set {
            let var = sub.vars.entry(&name);
            var.set(value);
            var.exported = true;
        }
        if let Some(cwd) = env.cwd {
            sub.cwd = cwd;
        }
        if let Some(path) = env.stdin {
            match self.vp.host().open(&path, OpenOptions::read()).await {
                Ok(f) => sub.fds.set(0, Rc::new(FdObj::File(f))),
                Err(_) => sub.fds.close(0),
            }
        }
        UtilChild {
            fut: Box::pin(async move {
                let mut sub = sub;
                let r = run_wrapped(&mut sub, argv, Vec::new()).await;
                let last = sub.status;
                r.unwrap_or_else(|u| u.status(last))
            }),
            status: None,
        }
    }

    /// Starts `sh -c SCRIPT` in psh (D1191): for `awk`'s `system()` and pipes, `sed`'s
    /// `e`, ... A fresh shell, as for a script the host cannot start (D1300): only the
    /// utility's environment as exported variables, no functions, aliases, traps or
    /// options; `$0` is `sh`. Its cwd, stderr and other descriptors are the utility's;
    /// stdin and stdout as `stdio` says. stdout is flushed first. It runs concurrently
    /// from now on, as its own task.
    pub async fn spawn_shell(&mut self, script: &[u8], stdio: ShellStdio) -> ShellChild {
        let _ = self.flush().await;
        let mut child = ShellChild {
            stdin: None,
            stdout: None,
            task: None,
            join: None,
            status: None,
        };
        let mut sub = match self.afresh().await {
            Ok(sub) => sub,
            Err(status) => {
                child.status = Some(status);
                return child;
            }
        };
        sub.opts.dash_c = true;
        sub.arg0 = b"sh".to_vec();
        sub.args = Vec::new();
        if stdio.stdin == ShellIo::Pipe {
            let (r, w) = crate::pipe::pipe();
            sub.fds.set(0, Rc::new(FdObj::PipeRead(r)));
            child.stdin = Some(UtilFile {
                file: Rc::new(FdObj::PipeWrite(w)),
            });
        }
        if stdio.stdout == ShellIo::Pipe {
            let (r, w) = crate::pipe::pipe();
            sub.fds.set(1, Rc::new(FdObj::PipeWrite(w)));
            child.stdout = Some(UtilFile {
                file: Rc::new(FdObj::PipeRead(r)),
            });
        }
        let script = script.to_vec();
        let sched = self.vp.core.sched.clone();
        let (id, join) = sched.spawn_with_id(async move {
            let (status, _) = crate::run_script(sub, crate::Input::Text(script)).await;
            status
        });
        child.task = Some((sched, id));
        child.join = Some(join);
        child
    }
}

impl UtilCtx<'_> {
    /// A new shell for what this utility runs, started afresh from its environment
    /// (D1300): a subshell's task state, its own variables. The nesting limit's status
    /// when it is reached.
    async fn afresh(&self) -> Result<Vp, i32> {
        let mut sub = crate::exec::enter_shared(self.vp)
            .await
            .map_err(|u| u.status(2))?;
        for (k, v) in &self.env {
            if let Ok(name) = std::str::from_utf8(k)
                && crate::state::is_name(k)
            {
                let var = sub.vars.entry(name);
                var.set(v.clone());
                var.exported = true;
            }
        }
        crate::exec::start_afresh(&mut sub);
        Ok(sub)
    }
}

/// What a wrapper runs: a standalone builtin, a utility, or an external.
async fn run_wrapped(vp: &mut Vp, argv: Vec<Vec<u8>>, assigns: Vec<(String, Vec<u8>)>) -> R {
    if argv.is_empty() {
        return Ok(0);
    }
    let name = argv[0].clone();
    if STANDALONE_BUILTINS.iter().any(|b| b.as_bytes() == name) {
        return crate::exec::run_builtin(vp, argv, assigns).await;
    }
    if let Some(e) = lookup(vp, &name).await {
        return Box::pin(run(vp, e, argv, assigns)).await;
    }
    crate::exec::run_external(vp, argv, assigns).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testhost::TestHost;
    use crate::{Script, Shell, Step};

    /// `upper`: stdin to stdout, upper-cased. `fake-cat` (F mode): the test host also
    /// has `cat`, so only the off-PATH name runs in-process. `wrap CMD...`: runs its
    /// arguments through `spawn` with `W=1` set. `rmish`: destructive.
    fn upper<'a>(ctx: &'a mut UtilCtx<'_>, _argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(async move {
            let mut buf = [0u8; 3];
            loop {
                match ctx.read(&mut buf).await {
                    Ok(0) => return 0,
                    Ok(n) => {
                        if ctx.write(&buf[..n].to_ascii_uppercase()).await.is_err() {
                            return 141;
                        }
                    }
                    Err(_) => return 1,
                }
            }
        })
    }

    fn says<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(async move {
            let name = ctx.name().to_string();
            let v = ctx.var(b"W").map(<[u8]>::to_vec).unwrap_or_default();
            let _ = ctx
                .write(
                    format!("{name} {} W={}\n", argv.len(), String::from_utf8_lossy(&v)).as_bytes(),
                )
                .await;
            ctx.error("to stderr").await;
            0
        })
    }

    fn wrap<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(async move {
            let env = SpawnEnv {
                set: vec![("W".into(), b"1".to_vec())],
                ..SpawnEnv::default()
            };
            let mut child = ctx.spawn(argv[1..].to_vec(), env).await;
            child.wait().await
        })
    }

    /// `shell SCRIPT`: `spawn_shell` with inherited stdio, its status. `shpipe SCRIPT
    /// N`: writes N lines `lineK` to its stdin pipe, then reads its stdout pipe and
    /// prints `[...]` around it and the status. `shdrop SCRIPT`: starts it and drops it.
    fn shell<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(async move {
            let _ = ctx.write(b"before\n").await;
            let stdio = ShellStdio {
                stdin: ShellIo::Inherit,
                stdout: ShellIo::Inherit,
            };
            let mut child = ctx.spawn_shell(&argv[1], stdio).await;
            child.wait().await
        })
    }

    fn shpipe<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(async move {
            let stdio = ShellStdio {
                stdin: ShellIo::Pipe,
                stdout: ShellIo::Pipe,
            };
            let mut child = ctx.spawn_shell(&argv[1], stdio).await;
            let n: usize = String::from_utf8_lossy(&argv[2]).parse().unwrap();
            let stdin = child.stdin.take().unwrap();
            for k in 0..n {
                stdin
                    .write_all(format!("line{k}\n").as_bytes())
                    .await
                    .unwrap();
            }
            drop(stdin);
            let mut out = Vec::new();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_end(&mut out)
                .await
                .unwrap();
            let st = child.wait().await;
            let _ = ctx.write(b"[").await;
            let _ = ctx.write(&out).await;
            let _ = ctx.write(format!("] {st}\n").as_bytes()).await;
            0
        })
    }

    fn shdrop<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
        Box::pin(async move {
            let stdio = ShellStdio {
                stdin: ShellIo::Pipe,
                stdout: ShellIo::Inherit,
            };
            let child = ctx.spawn_shell(&argv[1], stdio).await;
            ctx.sleep(Duration::from_millis(5)).await;
            drop(child);
            0
        })
    }

    static ENTRIES: &[UtilEntry] = &[
        UtilEntry {
            names: &["shell"],
            mode: Mode::Always,
            destructive: false,
            run: shell,
        },
        UtilEntry {
            names: &["shpipe"],
            mode: Mode::Always,
            destructive: false,
            run: shpipe,
        },
        UtilEntry {
            names: &["shdrop"],
            mode: Mode::Always,
            destructive: false,
            run: shdrop,
        },
        UtilEntry {
            names: &["upper"],
            mode: Mode::Always,
            destructive: false,
            run: upper,
        },
        UtilEntry {
            names: &["says", "cat"],
            mode: Mode::PathFallback,
            destructive: false,
            run: says,
        },
        UtilEntry {
            names: &["wrap"],
            mode: Mode::Always,
            destructive: false,
            run: wrap,
        },
        UtilEntry {
            names: &["rmish"],
            mode: Mode::Always,
            destructive: true,
            run: says,
        },
    ];

    fn run_with(src: &str, policy: UtilPolicy) -> (i32, String, String) {
        let host = Rc::new(TestHost::new());
        host.set_stdin(b"abc\ndef\n");
        let mut cfg = host.config();
        cfg.utilities = policy;
        let mut shell = Shell::new(cfg);
        shell.start(Script::command(src));
        let status = loop {
            match shell.step(7) {
                Step::Done(s) => break s,
                Step::Yield => {}
                other => panic!("{other:?} running {src:?}"),
            }
        };
        assert_eq!(shell.task_count(), 0);
        (
            status,
            String::from_utf8_lossy(&host.stdout()).into_owned(),
            String::from_utf8_lossy(&host.stderr()).into_owned(),
        )
    }

    fn on() -> UtilPolicy {
        UtilPolicy {
            entries: ENTRIES,
            ..UtilPolicy::default()
        }
    }

    #[test]
    fn dispatch_io_and_modes() {
        assert_eq!(run_with("upper", on()).1, "ABC\nDEF\n");
        assert_eq!(run_with("echo x | upper | upper", on()).1, "X\n");
        // F mode: `cat` is on PATH, so the program runs; `says` is not.
        assert_eq!(run_with("echo y | cat", on()).1, "y\n");
        assert_eq!(
            run_with("says a b", on()),
            (0, "says 3 W=\n".into(), "says: to stderr\n".into())
        );
        // A forced name runs the utility in F mode.
        let forced = UtilPolicy {
            force_names: vec![b"cat".to_vec()],
            ..on()
        };
        assert_eq!(run_with("cat", forced).1, "cat 1 W=\n");
        // A sandbox treats F as S.
        let sandbox = UtilPolicy {
            sandbox: true,
            ..on()
        };
        assert_eq!(run_with("cat", sandbox).1, "cat 1 W=\n");
        // A path always runs the program; functions win over utilities.
        assert_eq!(run_with("/bin/cat", on()).1, "abc\ndef\n");
        assert_eq!(run_with("upper() { echo fn; }; upper", on()).1, "fn\n");
        // Prefix assignments reach the utility's environment.
        assert_eq!(run_with("W=7 says", on()).1, "says 1 W=7\n");
    }

    #[test]
    fn spawn_shell_starts_afresh() {
        // Only exported variables (and the utility's prefix assignments); no
        // functions, aliases, traps or options; `$0` is `sh`; stdout flushed first.
        let (st, out, err) = run_with(
            "f() { echo fn; }; X=1; export Y=2; set -eu; trap 'echo T' EXIT; \
             Z=3 shell 'echo \"$0 $X-$Y-$Z\"; false; echo after; f' || echo $?",
            on(),
        );
        assert_eq!((st, out.as_str()), (0, "before\nsh -2-3\nafter\n127\nT\n"));
        assert_eq!(err, "sh: line 1: f: command not found\n");
        assert_eq!(run_with("shell 'exit 3'; echo $?", on()).1, "before\n3\n");
        assert_eq!(
            run_with("shell 'read a; echo got $a'", on()).1,
            "before\ngot abc\n"
        );
        assert_eq!(run_with("shell 'echo x | upper'", on()).1, "before\nX\n");
    }

    #[test]
    fn spawn_shell_pipes() {
        assert_eq!(
            run_with(
                "shpipe 'while read l; do echo \"<$l>\"; done; exit 4' 2",
                on()
            )
            .1,
            "[<line0>\n<line1>\n] 4\n"
        );
        // An external on the pipes.
        assert_eq!(
            run_with("shpipe 'cat; echo end' 2", on()).1,
            "[line0\nline1\nend\n] 0\n"
        );
        // More than a pipe holds goes through while the utility writes: the shell runs
        // concurrently.
        let (_, out, _) = run_with(
            "shpipe 'n=0; while read l; do n=$((n+1)); done; echo $n' 20000",
            on(),
        );
        assert_eq!(out, "[20000\n] 0\n");
        // Dropped before it ended: it is stopped (`run_with` checks no task is left).
        assert_eq!(
            run_with("shdrop 'while :; do :; done'; echo $?", on()).1,
            "0\n"
        );
    }

    #[test]
    fn off_switches() {
        let st = |src: &str, p: UtilPolicy| run_with(src, p).0;
        assert_eq!(
            st(
                "upper",
                UtilPolicy {
                    off_all: true,
                    ..on()
                }
            ),
            127
        );
        assert_eq!(
            st(
                "rmish",
                UtilPolicy {
                    off_destructive: true,
                    ..on()
                }
            ),
            127
        );
        assert_eq!(
            st(
                "upper",
                UtilPolicy {
                    off_destructive: true,
                    ..on()
                }
            ),
            0
        );
        assert_eq!(
            st(
                "upper",
                UtilPolicy {
                    off_names: vec![b"upper".to_vec()],
                    ..on()
                }
            ),
            127
        );
        assert_eq!(st("upper", UtilPolicy::default()), 127);
    }

    #[test]
    fn type_command_and_wrappers() {
        assert_eq!(
            run_with("type upper; command -v upper; type -t upper", on()).1,
            "upper is a psh utility\nupper\nbuiltin\n"
        );
        assert_eq!(run_with("command upper", on()).1, "ABC\nDEF\n");
        // `exec` into a utility: it runs, then the shell ends with its status.
        assert_eq!(
            run_with("exec upper; echo not reached", on()),
            (0, "ABC\nDEF\n".into(), String::new())
        );
        assert_eq!(run_with("(exec says); echo $?", on()).1, "says 1 W=\n0\n");
        // A wrapper runs utilities, standalone builtins and programs, not functions.
        assert_eq!(run_with("wrap says", on()).1, "says 1 W=1\n");
        assert_eq!(
            run_with("wrap echo hi; wrap true; echo $?", on()).1,
            "hi\n0\n"
        );
        assert_eq!(run_with("f() { echo fn; }; wrap f; echo $?", on()).0, 0);
        assert!(
            run_with("f() { echo fn; }; wrap f", on())
                .2
                .contains("f: command not found")
        );
        assert_eq!(run_with("echo q | wrap cat", on()).1, "q\n");
    }
}
