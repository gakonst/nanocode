//! The `Host` trait: every side effect of the engine goes through it (ARCH.md "The Host
//! trait", D1052).
//!
//! Everything that may wait is pollable: file system operations return a [`HostFuture`],
//! open files have `poll_read`/`poll_write`, external processes have `poll_wait`. The
//! engine polls them with a `Context` built from its own waker; a host that finishes an
//! operation on another thread wakes that waker. Nothing here may block the engine
//! thread on something another part of the same engine has to do first.
//!
//! Paths are bytes, absolute and `/`-separated as the shell sees them; the host turns
//! them into native paths at its boundary.
//!
//! Resources are released by `Drop`: dropping an [`ExtProc`] that is still running
//! abandons it (the host terminates it and reaps it in the background), dropping a
//! [`HostFile`] closes it. The engine cancels by dropping its tasks, so this is what
//! keeps a cancel from leaking processes or descriptors.

use std::any::Any;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

/// An operation the host may finish later.
pub type HostFuture<T> = Pin<Box<dyn Future<Output = T>>>;

/// A signal number. POSIX numbering on every platform; hosts map them (Windows: TERM and
/// INT become a console break, KILL terminates the job).
pub type Signal = i32;

pub const SIGHUP: Signal = 1;
pub const SIGINT: Signal = 2;
pub const SIGQUIT: Signal = 3;
pub const SIGKILL: Signal = 9;
pub const SIGPIPE: Signal = 13;
pub const SIGTERM: Signal = 15;
pub const SIGCHLD: Signal = 17;
pub const SIGCONT: Signal = 18;
pub const SIGSTOP: Signal = 19;

/// An open file, pipe end or terminal of the host. Shared between descriptors (`2>&1`
/// shares one object), hence `&self`.
pub trait HostFile: Any {
    /// Reads into `buf`; `Ready(Ok(0))` is end of file.
    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
    /// Writes from `buf`, at least one byte unless `buf` is empty. A pipe without readers
    /// is `ErrorKind::BrokenPipe`.
    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
    /// A terminal (`test -t`).
    fn is_terminal(&self) -> bool {
        false
    }
    /// What it is (`fstat`), where the host can tell: a regular file, a pipe, a device...
    /// (rg's "is stdin readable" heuristic, D1133). `None`: unknown.
    fn kind(&self) -> Option<FileKind> {
        None
    }
    fn seek(&self, _pos: io::SeekFrom) -> Option<io::Result<u64>> {
        None
    }
    fn is_null(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenOptions {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub create: bool,
    pub truncate: bool,
    /// Fail if the file exists (`set -o noclobber`).
    pub create_new: bool,
    /// Permission bits of a file this open creates, exactly (the shell's own umask is
    /// already applied by the engine). `None`: the host's default (`0o666` less the
    /// process's umask). A host applies them only to a file the open really created,
    /// never to an existing one, and never by changing the process's umask (other
    /// threads and the embedder share it); on Unix: create exclusively, then `fchmod`.
    pub mode: Option<u32>,
}

impl OpenOptions {
    pub fn read() -> OpenOptions {
        OpenOptions {
            read: true,
            ..OpenOptions::default()
        }
    }

    /// `>`: create or truncate.
    pub fn write() -> OpenOptions {
        OpenOptions {
            write: true,
            create: true,
            truncate: true,
            ..OpenOptions::default()
        }
    }

    /// `>>`
    pub fn append() -> OpenOptions {
        OpenOptions {
            write: true,
            append: true,
            create: true,
            ..OpenOptions::default()
        }
    }

    /// `<>`
    pub fn read_write() -> OpenOptions {
        OpenOptions {
            read: true,
            write: true,
            create: true,
            ..OpenOptions::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    Fifo,
    Socket,
    BlockDevice,
    CharDevice,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub kind: FileKind,
    pub size: u64,
    /// Permission bits including setuid, setgid and sticky (`0o7777`).
    pub mode: u32,
    /// Modification time in nanoseconds since the epoch.
    pub mtime_ns: i64,
    pub dev: u64,
    pub ino: u64,
    pub uid: u32,
    pub gid: u32,
}

/// The rest of `stat(2)`, for `ls -l`, `stat` and `du` (D1150). Separate from
/// [`Metadata`] so that hosts without it keep compiling: [`Host::stat_extra`] has a
/// default that derives it from `stat`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtraMetadata {
    /// Last access, nanoseconds since the epoch.
    pub atime_ns: i64,
    /// Last status change, nanoseconds since the epoch.
    pub ctime_ns: i64,
    /// Creation, where the file system records it.
    pub btime_ns: Option<i64>,
    /// Hard links.
    pub nlink: u64,
    /// Allocated size in 512-byte blocks (`du`, `ls -s`).
    pub blocks: u64,
    /// The preferred I/O block size.
    pub blksize: u64,
    /// The device a device file stands for.
    pub rdev: u64,
}

impl ExtraMetadata {
    /// What a host without the fields reports: times from `mtime`, one link, the size
    /// rounded up to blocks.
    pub fn derived(m: &Metadata) -> ExtraMetadata {
        ExtraMetadata {
            atime_ns: m.mtime_ns,
            ctime_ns: m.mtime_ns,
            btime_ns: None,
            nlink: 1,
            blocks: m.size.div_ceil(512),
            blksize: 4096,
            rdev: 0,
        }
    }
}

/// What `access` checks (`test -r/-w/-x`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: Vec<u8>,
}

/// A directory entry and its kind, where listing the directory told it.
pub type KnownEntry = (DirEntry, Option<FileKind>);

/// How an external process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    Exited(i32),
    Signaled(Signal),
}

impl ExitStatus {
    /// The shell's `$?` for it: the code, or `128 + signal`.
    pub fn code(self) -> i32 {
        match self {
            ExitStatus::Exited(c) => c,
            ExitStatus::Signaled(s) => 128 + s,
        }
    }
}

/// A descriptor of a process to spawn.
pub enum ChildFd {
    /// A file of this host, passed to the child as it is.
    File(Rc<dyn HostFile>),
    /// A new pipe: the child gets one end, the engine the other (in
    /// [`Spawned::pipes`]), which it pumps to or from an engine object. `input` is a
    /// pipe the child reads from.
    Pipe { input: bool },
    /// The same as an earlier descriptor of the list (`|&`, `2>&1` into a pipe: one
    /// pipe, so the order of the writes survives).
    Dup(u32),
    /// The null device.
    Null,
}

#[derive(Default)]
pub struct SpawnSpec {
    /// The program to execute, as resolved (`resolve`, or a name with a slash).
    pub program: Vec<u8>,
    /// The arguments, `argv[0]` first (the name as the user wrote it).
    pub argv: Vec<Vec<u8>>,
    /// The complete environment.
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub cwd: Vec<u8>,
    /// The child's descriptors: at least 0, 1 and 2, in any order.
    pub fds: Vec<(u32, ChildFd)>,
    /// Signals the child starts with ignored (bash: SIGINT and SIGQUIT in `&` jobs,
    /// `trap '' SIG`). Every other signal starts at its default disposition.
    pub sig_ignore: Vec<Signal>,
    /// The child's umask; `None` keeps the process's.
    pub umask: Option<u32>,
    /// Detached (`nohup`, `setsid`): the child leaves the shell's run. It gets a session
    /// of its own, is not killed with the run's leftovers, and dropping its `ExtProc`
    /// releases it instead of abandoning it.
    pub detach: bool,
    /// Resource limits for the child (`ulimit`), soft and hard; the others are the
    /// process's.
    pub rlimits: Vec<(Rlimit, Limit, Limit)>,
    pub group: bool,
}

/// A resource limit `ulimit` reports and sets. Values are in the resource's own unit:
/// bytes, seconds, microseconds (`RtTime`) or a count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rlimit {
    /// `-c`: core file size.
    Core,
    /// `-d`: data segment size.
    Data,
    /// `-e`: scheduling priority (nice).
    Nice,
    /// `-f`: file size.
    FileSize,
    /// `-i`: pending signals.
    SigPending,
    /// `-l`: locked memory.
    MemLock,
    /// `-m`: resident set size.
    Rss,
    /// `-n`: open files.
    NoFile,
    /// `-q`: POSIX message queue bytes.
    MsgQueue,
    /// `-r`: real-time priority.
    RtPrio,
    /// `-s`: stack size.
    Stack,
    /// `-t`: CPU time.
    Cpu,
    /// `-u`: user processes.
    NProc,
    /// `-v`: address space.
    As,
    /// `-x`: file locks.
    Locks,
    /// `-R`: real-time CPU time without blocking.
    RtTime,
}

/// A limit: `None` is unlimited.
pub type Limit = Option<u64>;

/// A running external process. Dropping it while it runs abandons it: the host sends
/// TERM, KILL after a grace period, and reaps it in the background. What a released
/// external leaves running is the run's leftovers: a host may kill them when the shell
/// is done (psh-os: by default, when the host is dropped).
pub trait ExtProc {
    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<ExitStatus>;
    fn poll_stop(&mut self, _cx: &mut Context<'_>) -> Poll<Signal> {
        Poll::Pending
    }
    fn signal(&mut self, sig: Signal);
    /// Lets the process run on: no signal, reaped in the background (`nohup`, and
    /// background jobs still running when the shell exits, as bash leaves them).
    fn release(self: Box<Self>);
    /// Like `release`, and the process also leaves the shell's run: it is not killed
    /// with the run's leftovers (`disown`).
    fn disown(self: Box<Self>) {
        self.release();
    }
    /// The OS process id (`$!`, `jobs -p`), where there is one.
    fn pid(&self) -> Option<u32> {
        None
    }
}

/// What `spawn` answers, inside an `io::Error`, for a program psh must run itself as a
/// shell script (D1300): a file the OS cannot start that bash would run in a copy of
/// itself. Unix: `execve` fails with `ENOEXEC` (no `#!`). Windows: `#!/bin/sh`,
/// `#!/bin/bash` (also through `/usr/bin/env`) and files with neither an interpreter line
/// nor an executable extension. The engine then reads `program` and runs it in a fresh
/// subshell with the same argv, environment, cwd and descriptors.
#[derive(Debug)]
pub struct RunInShell;

impl RunInShell {
    /// The error a host returns from `spawn`.
    pub fn error() -> io::Error {
        io::Error::other(RunInShell)
    }

    /// Whether `e` is that answer.
    pub fn is(e: &io::Error) -> bool {
        e.get_ref().is_some_and(|inner| inner.is::<RunInShell>())
    }
}

impl std::fmt::Display for RunInShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a shell script for psh to run itself")
    }
}

impl std::error::Error for RunInShell {}

/// What [`PreparedFork::fork`] made of the process that called it.
pub enum Forked {
    /// This is the process that forked: the child, to wait for like any external.
    Parent(Box<dyn ExtProc>),
    /// This is the copy: only the calling thread runs in it, and nothing the parent
    /// started is its own.
    Child,
}

/// A host ready to fork ([`Host::prepare_fork`]): it holds what its other threads
/// share, so the copy finds it consistent. The engine takes its own shared state
/// after this and calls `fork` at once.
pub trait PreparedFork {
    fn fork(self: Box<Self>) -> io::Result<Forked>;
}

pub struct Spawned {
    pub proc: Box<dyn ExtProc>,
    /// The engine's ends of the `ChildFd::Pipe` descriptors, by child fd.
    pub pipes: Vec<(u32, Rc<dyn HostFile>)>,
}

/// Maps an absolute host path to the OS path (see [`Host::native_paths`]).
pub type NativePathFn = fn(&[u8]) -> io::Result<std::path::PathBuf>;

/// Facts about the process and the machine that bash reports in variables (D1301):
/// The CPU time a process has used in user and in system mode, as `getrusage` counts
/// it: the shell's own, and that of the children it has waited for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub user: Duration,
    pub system: Duration,
    pub children_user: Duration,
    pub children_system: Duration,
}

/// `UID`, `EUID`, `GROUPS`, `PPID`, `HOSTNAME`, `OSTYPE`, `MACHTYPE`, `HOSTTYPE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub uid: u32,
    pub euid: u32,
    /// The supplementary groups, the real group first (bash's `GROUPS`).
    pub groups: Vec<u32>,
    /// The shell's parent process; `None` where there is no such thing.
    pub ppid: Option<u32>,
    pub hostname: Vec<u8>,
    /// As bash reports them: `linux-gnu`, `darwin25.6.0`; `msys` proposed for
    /// Windows (agents' scripts take their Windows branch on it, and psh accepts
    /// `/c/...` paths).
    pub ostype: String,
    /// `x86_64-pc-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin25.6.0`.
    pub machtype: String,
    /// The processor: `x86_64`, `aarch64`.
    pub hosttype: String,
    pub release: String,
    pub version: String,
}

impl Default for HostInfo {
    /// For hosts without an OS underneath (the test host, WASM): a plain user on an
    /// unknown system.
    fn default() -> Self {
        let arch = std::env::consts::ARCH.to_string();
        HostInfo {
            uid: 1000,
            euid: 1000,
            groups: vec![1000],
            ppid: None,
            hostname: b"localhost".to_vec(),
            ostype: "unknown".into(),
            machtype: format!("{arch}-unknown-unknown"),
            hosttype: arch,
            release: String::new(),
            version: String::new(),
        }
    }
}

/// The engine's view of the outside world. Minimal on purpose; it grows with the
/// features that need it (ARCH.md lists what is planned).
pub trait Host {
    /// Opens a file. The path is absolute.
    fn open(&self, path: &[u8], opts: OpenOptions) -> HostFuture<io::Result<Rc<dyn HostFile>>>;
    /// `stat` (`follow`) or `lstat`.
    fn stat(&self, path: &[u8], follow: bool) -> HostFuture<io::Result<Metadata>>;
    /// Whether the current user may read, write or execute the file.
    fn access(&self, path: &[u8], mode: Access) -> HostFuture<io::Result<bool>>;
    /// The entries of a directory, without `.` and `..`, in no particular order.
    fn read_dir(&self, path: &[u8]) -> HostFuture<io::Result<Vec<DirEntry>>>;
    /// [`Host::read_dir`] with the kind of each entry where listing the directory
    /// tells it (a link is a link, not what it points to), `None` where only `stat`
    /// does: a walk that needs nothing else about a file then asks nothing more. The
    /// default knows no kind.
    fn read_dir_kinds(&self, path: &[u8]) -> HostFuture<io::Result<Vec<KnownEntry>>> {
        let listed = self.read_dir(path);
        Box::pin(async move { Ok(listed.await?.into_iter().map(|e| (e, None)).collect()) })
    }
    /// The physical path: symlinks, `.` and `..` resolved (`pwd -P`, `cd -P`).
    fn canonicalize(&self, path: &[u8]) -> HostFuture<io::Result<Vec<u8>>>;
    /// Finds a command name (no slash) in `path` (the shell's `PATH`, relative entries
    /// against `cwd`): the program to spawn, or `None`.
    fn resolve(&self, name: &[u8], path: &[u8], cwd: &[u8]) -> HostFuture<Option<Vec<u8>>>;
    /// The entries of a `PATH`-like value, in order: what `resolve` and the lookups of
    /// `type`, `hash` and `source` try one at a time. POSIX splits on `:`; the Windows
    /// host splits on `;`, and on `:` except after a drive letter (D1910).
    fn path_entries(&self, path: &[u8]) -> Vec<Vec<u8>> {
        path.split(|&b| b == b':').map(<[u8]>::to_vec).collect()
    }
    /// Starts an external process. Errors are the reasons it could not start
    /// (`NotFound`, `PermissionDenied`, ...).
    fn spawn(&self, spec: SpawnSpec) -> io::Result<Spawned>;
    /// Whether a program this host starts can be given `file` as a descriptor
    /// ([`ChildFd::File`]). A file that is not the host's own (an embedder's sink as
    /// the shell's stdout) cannot: the engine gives the program a pipe and moves the
    /// bytes itself.
    fn inherits(&self, _file: &dyn HostFile) -> bool {
        true
    }
    /// A new OS pipe: (read end, write end). Both are files of this host: the engine
    /// may read and write them like any file and pass them to externals as
    /// [`ChildFd::File`]. Hosts without OS pipes keep the default.
    fn pipe(&self) -> io::Result<(Rc<dyn HostFile>, Rc<dyn HostFile>)> {
        Err(io::ErrorKind::Unsupported.into())
    }
    /// A host whose shell is its own process, like bash, and may fork it: a background
    /// job that is not one program then runs in a copy of the process and outlives
    /// the shell, as a job of bash does (D1900). `None`, the default, keeps such a job
    /// a task of this process, which ends with the script.
    fn prepare_fork(&self) -> Option<Box<dyn PreparedFork>> {
        None
    }
    /// Whether [`Host::prepare_fork`] answers.
    fn forks(&self) -> bool {
        false
    }
    /// Starts a thread of the host's process that runs `body`: a blocking utility's
    /// worker (D1130). A host that forks starts it so that a fork meanwhile leaves the
    /// copy able to start threads of its own.
    fn thread(&self, name: String, body: Box<dyn FnOnce() + Send>) -> io::Result<()> {
        std::thread::Builder::new().name(name).spawn(body).map(drop)
    }
    /// Sends `sig` to process `pid` (negative: a process group), as `kill(2)`: the
    /// `kill` builtin's numeric pids.
    fn kill(&self, pid: i32, sig: Signal) -> io::Result<()> {
        let _ = (pid, sig);
        Err(io::ErrorKind::Unsupported.into())
    }
    /// When this host's paths name the process's own file system: how a path becomes
    /// the OS path `std::fs` takes (D1130). Blocking utilities then run on a worker
    /// thread and use `std::fs` there directly; hosts with a file system of their own
    /// (wasm, sandboxes, tests) keep the default `None`, and those utilities run inline
    /// over the Host. A plain function, so other threads can call it.
    fn native_paths(&self) -> Option<NativePathFn> {
        None
    }
    /// The system's standard `PATH`, where the standard utilities are (`command -p`):
    /// POSIX's `confstr(_CS_PATH)` on Unix. It differs per system (`/bin:/usr/bin` on
    /// Linux), so it comes from the host.
    fn standard_path(&self) -> Vec<u8> {
        b"/usr/bin:/bin:/usr/sbin:/sbin".to_vec()
    }
    /// User, group, parent and machine facts for the shell's variables (D1301). Asked
    /// once per shell.
    fn info(&self) -> HostInfo {
        HostInfo::default()
    }
    /// Whether lines from this host's programs and files end in `\r\n` as a rule
    /// (Windows): `read` then drops a `\r` right before the newline, and command
    /// substitution one before each trailing newline it removes (D1292).
    fn strips_crlf(&self) -> bool {
        false
    }
    /// The process's soft and hard limit of a resource; `None` when the platform has no
    /// such limit (`ulimit` then rejects its option, as bash does there).
    fn rlimit(&self, resource: Rlimit) -> Option<(Limit, Limit)> {
        let _ = resource;
        None
    }
    /// The process's file mode creation mask when the host was made: where the shell's
    /// own mask (`umask`) starts. Hosts without one report `0o022`.
    fn umask(&self) -> u32 {
        0o022
    }
    /// `stat` plus the rest of `stat(2)` (D1150). The default derives the rest from
    /// `stat` ([`ExtraMetadata::derived`]).
    fn stat_extra(
        &self,
        path: &[u8],
        follow: bool,
    ) -> HostFuture<io::Result<(Metadata, ExtraMetadata)>> {
        let fut = self.stat(path, follow);
        Box::pin(async move {
            let m = fut.await?;
            Ok((m, ExtraMetadata::derived(&m)))
        })
    }

    // File system changes (D1150). Each defaults to `ErrorKind::Unsupported`, which a
    // utility reports as `Operation not supported`; paths are absolute like `open`'s.

    /// Creates one directory with exactly the permission bits `mode` (the caller has
    /// applied the shell's umask; the host does not apply the process's). An existing
    /// path is `AlreadyExists`; a missing parent `NotFound`.
    fn mkdir(&self, path: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        let _ = (path, mode);
        unsupported()
    }
    /// Removes a name that is not a directory (`unlink`): a file, or a symlink itself
    /// (on Windows also a symlink or junction to a directory, which is never followed).
    fn remove_file(&self, path: &[u8]) -> HostFuture<io::Result<()>> {
        let _ = path;
        unsupported()
    }
    /// Removes an empty directory (`rmdir`).
    fn remove_dir(&self, path: &[u8]) -> HostFuture<io::Result<()>> {
        let _ = path;
        unsupported()
    }
    /// `rename(2)`: replaces `to` when it is a file (or an empty directory and `from`
    /// is one). Across file systems: `ErrorKind::CrossesDevices`.
    fn rename(&self, from: &[u8], to: &[u8]) -> HostFuture<io::Result<()>> {
        let _ = (from, to);
        unsupported()
    }
    /// Creates the symlink `link` pointing to `target`, which is stored as given
    /// (relative to the link's directory unless absolute; it need not exist). A host
    /// that cannot make symlinks may make an equivalent (a Windows junction for a
    /// directory), never a copy.
    fn symlink(&self, target: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        let _ = (target, link);
        unsupported()
    }
    /// Creates the hard link `link` to the existing file `existing`.
    fn hard_link(&self, existing: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        let _ = (existing, link);
        unsupported()
    }
    /// A symlink's target as stored. Not a symlink: `ErrorKind::InvalidInput`.
    fn read_link(&self, path: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        let _ = path;
        unsupported()
    }
    /// `chmod`: sets the permission bits (`0o7777`), following symlinks. A host maps
    /// what it can (Windows: the write bits are the read-only attribute).
    fn set_mode(&self, path: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        let _ = (path, mode);
        unsupported()
    }
    /// Sets access and modification times, nanoseconds since the epoch; `None` leaves
    /// that time as it is. `follow`: the symlink's target, else the link itself.
    fn set_times(
        &self,
        path: &[u8],
        atime_ns: Option<i64>,
        mtime_ns: Option<i64>,
        follow: bool,
    ) -> HostFuture<io::Result<()>> {
        let _ = (path, atime_ns, mtime_ns, follow);
        unsupported()
    }
    /// The name of a user id (`ls -l`, `stat %U`), if the host knows it.
    fn user_name(&self, uid: u32) -> Option<Vec<u8>> {
        let _ = uid;
        None
    }
    /// The name of a group id, if the host knows it.
    fn group_name(&self, gid: u32) -> Option<Vec<u8>> {
        let _ = gid;
        None
    }
    /// The local time's offset east of UTC, in seconds, at `epoch_secs` (the process's
    /// time zone). The default is UTC.
    fn utc_offset(&self, epoch_secs: i64) -> i32 {
        let _ = epoch_secs;
        0
    }
    /// The current time as a duration since the Unix epoch (`printf '%(fmt)T'`, `$EPOCHSECONDS`).
    /// The default is the epoch itself: hosts with a clock override it (wasm32 has no
    /// `SystemTime`; the JS host supplies `Date.now()`).
    fn now(&self) -> Duration {
        Duration::ZERO
    }
    /// The CPU time used so far (`time`, `times`). The default is none: a host
    /// without processes of its own has no such clock.
    fn cpu_times(&self) -> CpuTimes {
        CpuTimes::default()
    }
    /// Completes after `duration`.
    fn sleep(&self, duration: Duration) -> HostFuture<()>;
    /// The shell's process id (`$$`).
    fn pid(&self) -> u32;
    /// The home directory of a user (`~user`), if the host knows it.
    fn home_dir(&self, user: &[u8]) -> Option<Vec<u8>> {
        let _ = user;
        None
    }
    /// A command name that is no function, builtin, utility or program, told before
    /// "command not found" is printed: a host without programs learns what a script
    /// needed (nanocode-shell stops the run on it when asked to).
    fn not_found(&self, name: &[u8]) {
        let _ = name;
    }
}

fn unsupported<T: 'static>() -> HostFuture<io::Result<T>> {
    Box::pin(std::future::ready(Err(io::ErrorKind::Unsupported.into())))
}

/// The text bash prints for an error (`strerror`), without Rust's `(os error N)`.
pub fn error_text(err: &io::Error) -> String {
    use io::ErrorKind::*;
    let s = match err.kind() {
        NotFound => "No such file or directory",
        PermissionDenied => "Permission denied",
        AlreadyExists => "File exists",
        BrokenPipe => "Broken pipe",
        IsADirectory => "Is a directory",
        NotADirectory => "Not a directory",
        DirectoryNotEmpty => "Directory not empty",
        ReadOnlyFilesystem => "Read-only file system",
        InvalidFilename => "File name too long",
        Interrupted => "Interrupted system call",
        Unsupported => "Operation not supported",
        CrossesDevices => "Invalid cross-device link",
        InvalidInput => "Invalid argument",
        _ => {
            let text = err.to_string();
            return match text.find(" (os error") {
                Some(i) => text[..i].to_string(),
                None => text,
            };
        }
    };
    s.to_string()
}
