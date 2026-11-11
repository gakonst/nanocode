//! The Windows host (ARCH.md "psh-os", Windows part and "Windows rules").
//!
//! - Processes: [`spawn`] (`CreateProcessW` suspended, a Job Object per external,
//!   only the stdio handles inherited, waiter threads, ctrl-break then
//!   `TerminateJobObject` on abandon).
//! - Files: [`WinFile`], whose I/O never waits for another part of the engine: pump
//!   pipes run on helper threads, shared pipes are read only as far as data is there
//!   ([`poller`]).
//! - Paths: the engine's `C:/x` (also `/c/x`, `/dev/null`, `/tmp`) at the boundary
//!   ([`path`]); command lookup with `PATHEXT`.
//! - Timers on the shared timer thread.
//!
//! Embedded, the host installs no handlers and changes no process-wide state (it never
//! allocates or changes a console).

mod cmdline;
mod file;
mod fs;
mod path;
mod poller;
pub mod signals;
mod spawn;

use std::io;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use psh_engine::HostFile;
use psh_engine::host::{
    Access, DirEntry, FileKind, Host, HostFuture, Metadata, OpenOptions, Signal, SpawnSpec, Spawned,
};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandle,
};

pub use crate::Leftovers;
pub use file::{Decode, WinFile};
pub use spawn::{MissingInterpreter, Placement, settle};

use spawn::Shared;

/// Owns a handle a Win32 call returned (null or `INVALID_HANDLE_VALUE` = the error).
pub(crate) fn owned(h: HANDLE) -> io::Result<OwnedHandle> {
    if h.is_null() || h as isize == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle the call returned to us, owned by nobody else.
    Ok(unsafe { OwnedHandle::from_raw_handle(h as RawHandle) })
}

/// Settings of a [`WindowsHost`].
#[derive(Clone, Copy, Debug)]
pub struct WindowsConfig {
    pub placement: Placement,
    /// Time between TERM (a ctrl-break) and termination when a running external is
    /// abandoned.
    pub grace: Duration,
    /// `$$`, when it must not be the process's own id (an embedder running shells
    /// in-process).
    pub pid: Option<u32>,
    /// What happens to the run's leftovers when the host is dropped.
    pub leftovers: Leftovers,
    /// How output of native programs read by the engine is decoded.
    pub decode: Decode,
    /// Detached externals (`nohup`, `setsid`) outlive the run. Off for a test runner,
    /// where nothing a case started may survive it: their jobs then belong to the run.
    pub detached_survive: bool,
}

impl WindowsConfig {
    pub const DEFAULT_GRACE: Duration = Duration::from_secs(2);
}

impl Default for WindowsConfig {
    fn default() -> Self {
        WindowsConfig {
            placement: Placement::Group,
            grace: WindowsConfig::DEFAULT_GRACE,
            pid: None,
            leftovers: Leftovers::Kill,
            decode: Decode::Off,
            detached_survive: true,
        }
    }
}

/// The host. One per shell: everything it spawned is that shell's run, and dropping
/// it ends the run ([`Leftovers`]).
pub struct WindowsHost {
    shared: Arc<Shared>,
    pid: Option<u32>,
    leftovers: Leftovers,
}

/// Kills a run's leftovers from any thread ([`WindowsHost::killer`]).
#[derive(Clone)]
pub struct RunKiller(Arc<Shared>);

impl RunKiller {
    pub fn kill_all(&self) {
        self.0.kill_all();
    }

    /// Like [`WindowsHost::leftovers`], also after the host is gone (leak checks).
    pub fn leftovers(&self) -> Vec<i32> {
        let jobs = self.0.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter()
            .filter(|j| j.active() > 0)
            .map(|j| j.pid() as i32)
            .collect()
    }
}

impl Drop for WindowsHost {
    fn drop(&mut self) {
        match self.leftovers {
            Leftovers::Kill => self.shared.kill_all(),
            Leftovers::Keep => self.shared.keep_all(),
        }
    }
}

impl Default for WindowsHost {
    fn default() -> Self {
        WindowsHost::new(WindowsConfig::default())
    }
}

impl WindowsHost {
    pub fn new(cfg: WindowsConfig) -> WindowsHost {
        WindowsHost {
            shared: Arc::new(Shared {
                grace: cfg.grace,
                placement: cfg.placement,
                console: spawn::has_console(),
                decode: cfg.decode,
                detached_survive: cfg.detached_survive,
                jobs: Mutex::new(Vec::new()),
            }),
            pid: cfg.pid,
            leftovers: cfg.leftovers,
        }
    }

    /// Kills the run's leftovers now (see [`Leftovers::Kill`]). Returns at once;
    /// [`settle`] waits.
    pub fn kill_all(&self) {
        self.shared.kill_all();
    }

    /// A handle that kills the run's leftovers from any thread.
    pub fn killer(&self) -> RunKiller {
        RunKiller(self.shared.clone())
    }

    /// The pids of every external of the run recorded so far, live or not.
    pub fn groups(&self) -> Vec<i32> {
        let jobs = self.shared.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter().map(|j| j.pid() as i32).collect()
    }

    /// The externals of the run whose job still has a live process (released externals
    /// included, detached and disowned ones not). Empty after a run that leaked nothing,
    /// once abandoned externals have been killed (up to the grace period).
    pub fn leftovers(&self) -> Vec<i32> {
        let jobs = self.shared.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter()
            .filter(|j| j.active() > 0)
            .map(|j| j.pid() as i32)
            .collect()
    }
}

fn ready<T: 'static>(v: T) -> HostFuture<T> {
    Box::pin(std::future::ready(v))
}

/// `PATHEXT`, lower case with the dot (`.com`, `.exe`, ...), from the process's
/// environment.
pub(crate) fn pathext() -> Vec<String> {
    let v = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    v.split(';')
        .filter(|e| e.len() > 1 && e.starts_with('.'))
        .map(|e| e.to_ascii_lowercase())
        .collect()
}

/// Whether the file starts with `#!`.
fn has_shebang(p: &std::path::Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 2];
    std::fs::File::open(p).is_ok_and(|mut f| f.read_exact(&mut head).is_ok() && head == *b"#!")
}

/// A file a command name can resolve to: a regular file, or an App Execution Alias
/// (the 0-byte reparse points in WindowsApps, like `winget.exe`), which std's metadata
/// does not see as a file.
fn is_command_file(p: &std::path::Path) -> bool {
    match std::fs::metadata(p) {
        Ok(m) => m.is_file(),
        Err(_) => std::fs::symlink_metadata(p)
            .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0),
    }
}

/// Looks `name` up in the `PATH` value `path_var` (D1076): for each entry, `name` as it
/// is when it already has a `PATHEXT` extension, else `name` plus each extension of
/// `PATHEXT` in order, then the bare name if it is a `#!` script. Empty and relative
/// entries are relative to `cwd`; the cwd is never searched implicitly.
pub(crate) fn resolve_in(name: &[u8], path_var: &[u8], cwd: &[u8]) -> Option<Vec<u8>> {
    let name_s = std::str::from_utf8(name).ok()?;
    let exts = pathext();
    let lower = name_s.to_ascii_lowercase();
    let has_ext = exts.iter().any(|e| lower.ends_with(e.as_str()));
    for dir in path::split_path_var(path_var) {
        let mut base = if path::is_absolute(dir) {
            dir.to_vec()
        } else {
            let mut abs = cwd.to_vec();
            if !dir.is_empty() {
                abs.push(b'/');
                abs.extend_from_slice(dir);
            }
            abs
        };
        while base.len() > 1 && matches!(base.last(), Some(b'/' | b'\\')) {
            base.pop();
        }
        base.push(b'/');
        base.extend_from_slice(name);
        let Ok(native) = path::native(&base) else {
            continue;
        };
        if has_ext && is_command_file(&native) {
            return Some(path::engine_path(&native));
        }
        for e in &exts {
            let mut cand = native.clone().into_os_string();
            cand.push(e);
            let cand = std::path::PathBuf::from(cand);
            if is_command_file(&cand) {
                return Some(path::engine_path(&cand));
            }
        }
        if !has_ext && std::fs::metadata(&native).is_ok_and(|m| m.is_file()) && has_shebang(&native)
        {
            return Some(path::engine_path(&native));
        }
    }
    None
}

/// File identity (volume serial and file index), for `-ef` and loop checks.
fn file_id(p: &std::path::Path, follow: bool) -> Option<(u64, u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
    if !follow {
        flags |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    let f = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(flags)
        .open(p)
        .ok()?;
    // SAFETY: plain data, filled by the call.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: an open handle and a writable struct.
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as HANDLE, &raw mut info) } == 0 {
        return None;
    }
    let ino = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some((
        u64::from(info.dwVolumeSerialNumber),
        ino,
        u64::from(info.nNumberOfLinks),
    ))
}

fn is_executable(p: &std::path::Path, m: &std::fs::Metadata) -> bool {
    if m.is_dir() {
        return true;
    }
    let lower = p.to_string_lossy().to_ascii_lowercase();
    pathext().iter().any(|e| lower.ends_with(e.as_str())) || has_shebang(p)
}

fn metadata(p: &std::path::Path, m: &std::fs::Metadata, follow: bool) -> Metadata {
    let ft = m.file_type();
    let kind = if ft.is_symlink() {
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
    let mut mode = if m.is_dir() { 0o755 } else { 0o644 };
    if !m.is_dir() && is_executable(p, m) {
        mode |= 0o111;
    }
    if m.file_attributes() & FILE_ATTRIBUTE_READONLY != 0 {
        mode &= !0o222;
    }
    let (dev, ino, _) = file_id(p, follow).unwrap_or((0, 0, 1));
    // Owned by the shell's user (HostInfo's 1000): `test -O` and `ls -l` agree.
    Metadata {
        kind,
        // NTFS directories have no size; Unix file systems report a block (Linux 4096),
        // and `test -s DIR` is true there.
        size: if m.is_dir() { 4096 } else { m.len() },
        mode,
        mtime_ns,
        dev,
        ino,
        uid: 1000,
        gid: 1000,
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

/// Opens `p` (D1911). Windows keeps one permission bit, the read-only attribute: a file
/// the open creates with `opts.mode` lacking every write bit gets it, set through the
/// open handle, which can still write (like `open(2)` with mode 0444). Every other bit of
/// `mode` has no Windows meaning; an existing file keeps its attributes.
fn open_file(p: &std::path::Path, opts: OpenOptions) -> io::Result<std::fs::File> {
    let read_only = opts.mode.is_some_and(|m| opts.create && m & 0o222 == 0);
    if !read_only {
        return std_open_options(opts).open(p);
    }
    loop {
        let mut create = std_open_options(OpenOptions {
            truncate: false,
            ..opts
        });
        create.create_new(true);
        match create.open(p) {
            Ok(f) => {
                let mut perm = f.metadata()?.permissions();
                perm.set_readonly(true);
                f.set_permissions(perm)?;
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

impl Host for WindowsHost {
    fn native_paths(&self) -> Option<psh_engine::host::NativePathFn> {
        Some(path::native)
    }

    fn open(&self, p: &[u8], opts: OpenOptions) -> HostFuture<io::Result<Rc<dyn HostFile>>> {
        let dev = match p {
            b"/dev/zero" => Some(file::Dev::Zero),
            b"/dev/urandom" | b"/dev/random" => Some(file::Dev::Random),
            _ => None,
        };
        if let Some(d) = dev {
            return ready(Ok(Rc::new(WinFile::dev(d)) as Rc<dyn HostFile>));
        }
        let r = path::native(p).and_then(|np| {
            // A directory opened for reading: Unix opens it and fails the reads with
            // "Is a directory"; Windows refuses the open with "Access denied".
            if opts.read && !opts.write && std::fs::metadata(&np).is_ok_and(|m| m.is_dir()) {
                return Ok(Rc::new(WinFile::dev(file::Dev::Dir)) as Rc<dyn HostFile>);
            }
            open_file(&np, opts)
                .map(|f| Rc::new(WinFile::new(OwnedHandle::from(f))) as Rc<dyn HostFile>)
        });
        ready(r)
    }

    fn stat(&self, p: &[u8], follow: bool) -> HostFuture<io::Result<Metadata>> {
        // The devices the host provides are character devices, as on Unix.
        if std::str::from_utf8(p).is_ok_and(path::is_null_device)
            || matches!(p, b"/dev/zero" | b"/dev/urandom" | b"/dev/random")
        {
            return ready(Ok(Metadata {
                kind: FileKind::CharDevice,
                size: 0,
                mode: 0o666,
                mtime_ns: 0,
                dev: 0,
                ino: 0,
                uid: 0,
                gid: 0,
            }));
        }
        let r = path::native(p).and_then(|np| {
            let m = if follow {
                std::fs::metadata(&np)
            } else {
                std::fs::symlink_metadata(&np)
            }?;
            Ok(metadata(&np, &m, follow))
        });
        ready(r)
    }

    fn access(&self, p: &[u8], mode: Access) -> HostFuture<io::Result<bool>> {
        let r = path::native(p).and_then(|np| {
            let m = match std::fs::metadata(&np) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(e) => return Err(e),
            };
            Ok(match mode {
                Access::Read => m.is_dir() || std::fs::File::open(&np).is_ok(),
                Access::Write => m.file_attributes() & FILE_ATTRIBUTE_READONLY == 0,
                Access::Execute => is_executable(&np, &m),
            })
        });
        ready(r)
    }

    fn read_dir(&self, p: &[u8]) -> HostFuture<io::Result<Vec<DirEntry>>> {
        let r = path::native(p).and_then(|np| {
            Ok(std::fs::read_dir(np)?
                .filter_map(Result::ok)
                .map(|e| DirEntry {
                    name: e.file_name().to_string_lossy().into_owned().into_bytes(),
                })
                .collect())
        });
        ready(r)
    }

    fn canonicalize(&self, p: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        let r = path::native(p)
            .and_then(std::fs::canonicalize)
            .map(|c| path::engine_path(&c));
        ready(r)
    }

    fn path_entries(&self, path_var: &[u8]) -> Vec<Vec<u8>> {
        path::split_path_var(path_var)
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect()
    }

    fn resolve(&self, name: &[u8], path_var: &[u8], cwd: &[u8]) -> HostFuture<Option<Vec<u8>>> {
        ready(resolve_in(name, path_var, cwd))
    }

    fn spawn(&self, spec: SpawnSpec) -> io::Result<Spawned> {
        spawn::spawn(&self.shared, spec)
    }

    fn pipe(&self) -> io::Result<(Rc<dyn HostFile>, Rc<dyn HostFile>)> {
        let (r, w) = file::os_pipe()?;
        Ok((Rc::new(r), Rc::new(w)))
    }

    fn kill(&self, pid: i32, sig: Signal) -> io::Result<()> {
        let me = std::process::id() as i32;
        // Embedded (a fake `$$`), the process is the embedder's.
        if self.pid.is_some() && (pid == me || pid == -me) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        // 0 and -1: every external of the run (psh's own group, which has no other
        // meaning here).
        if pid == 0 || pid == -1 {
            let jobs: Vec<_> = self
                .shared
                .jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            for j in jobs.iter().filter(|j| j.active() > 0) {
                let _ = j.signal(sig);
            }
            return Ok(());
        }
        // An external of the run (a negative pid: its group, which is its job).
        if let Some(job) = self.shared.job_of(pid.unsigned_abs()) {
            return job.signal(sig);
        }
        if pid < 0 {
            return Err(io::Error::from_raw_os_error(87));
        }
        spawn::kill_pid(pid as u32, sig)
    }

    fn stat_extra(
        &self,
        p: &[u8],
        follow: bool,
    ) -> HostFuture<io::Result<(Metadata, psh_engine::host::ExtraMetadata)>> {
        let r = path::native(p).and_then(|np| {
            let m = if follow {
                std::fs::metadata(&np)
            } else {
                std::fs::symlink_metadata(&np)
            }?;
            let links = file_id(&np, follow).map_or(1, |(_, _, n)| n);
            Ok((metadata(&np, &m, follow), fs::extra(&m, links)))
        });
        ready(r)
    }

    fn mkdir(&self, p: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        ready(path::native(p).and_then(|np| fs::mkdir(&np, mode)))
    }

    fn remove_file(&self, p: &[u8]) -> HostFuture<io::Result<()>> {
        ready(path::native(p).and_then(|np| fs::remove_file(&np)))
    }

    fn remove_dir(&self, p: &[u8]) -> HostFuture<io::Result<()>> {
        ready(path::native(p).and_then(std::fs::remove_dir))
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> HostFuture<io::Result<()>> {
        ready(path::native(from).and_then(|f| fs::rename(&f, &path::native(to)?)))
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        ready(path::native(link).and_then(|l| fs::symlink(target, &l)))
    }

    fn hard_link(&self, existing: &[u8], link: &[u8]) -> HostFuture<io::Result<()>> {
        ready(path::native(existing).and_then(|e| std::fs::hard_link(e, path::native(link)?)))
    }

    fn read_link(&self, p: &[u8]) -> HostFuture<io::Result<Vec<u8>>> {
        ready(path::native(p).and_then(|np| fs::read_link(&np)))
    }

    fn set_mode(&self, p: &[u8], mode: u32) -> HostFuture<io::Result<()>> {
        ready(path::native(p).and_then(|np| fs::set_mode(&np, mode)))
    }

    fn set_times(
        &self,
        p: &[u8],
        atime_ns: Option<i64>,
        mtime_ns: Option<i64>,
        follow: bool,
    ) -> HostFuture<io::Result<()>> {
        ready(path::native(p).and_then(|np| fs::set_times(&np, atime_ns, mtime_ns, follow)))
    }

    fn user_name(&self, uid: u32) -> Option<Vec<u8>> {
        (uid == 1000).then(fs::user).flatten()
    }

    fn utc_offset(&self, epoch_secs: i64) -> i32 {
        fs::utc_offset(epoch_secs)
    }

    fn home_dir(&self, user: &[u8]) -> Option<Vec<u8>> {
        let me = fs::user()?;
        if !me.eq_ignore_ascii_case(user) {
            return None;
        }
        let home = std::env::var_os("USERPROFILE")?;
        Some(path::engine_path(std::path::Path::new(&home)))
    }

    /// Windows' own default search path for a fresh account (D1917): System32, the
    /// Windows directory, WMI and Windows PowerShell's, in psh's form (`/`, `;`).
    fn standard_path(&self) -> Vec<u8> {
        let root = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .or_else(|| spawn::system_dir().map(|d| d.parent().map(Into::into).unwrap_or(d)))
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
        let dirs = [
            root.join("system32"),
            root.clone(),
            root.join(r"System32\Wbem"),
            root.join(r"System32\WindowsPowerShell\v1.0"),
        ];
        let mut out = Vec::new();
        for (i, d) in dirs.iter().enumerate() {
            if i > 0 {
                out.push(b';');
            }
            out.extend(path::engine_path(d));
        }
        out
    }

    fn info(&self) -> psh_engine::host::HostInfo {
        info(self.pid.is_some())
    }

    /// Windows has no umask: bash's usual default, so `umask` shows what scripts expect
    /// (D1911). A spawn's `umask` has nothing to apply to.
    fn umask(&self) -> u32 {
        0o022
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
}

/// The process and machine facts (D1913), as Git Bash reports them where bash has a
/// Windows form: `OSTYPE` `msys`, `MACHTYPE` `x86_64-pc-msys` (`aarch64-pc-msys`), so
/// scripts take their Windows branch. Windows users have no numeric ids: 1000. The
/// parent from a process snapshot; embedded (a fake `$$`), the embedding process.
fn info(fake_pid: bool) -> psh_engine::host::HostInfo {
    use windows_sys::Win32::System::SystemInformation::{
        ComputerNamePhysicalDnsHostname, GetComputerNameExW,
    };
    let arch = std::env::consts::ARCH.to_string();
    let mut name = vec![0u16; 256];
    let mut len = name.len() as u32;
    // SAFETY: `name` is writable for `len` units.
    let hostname = if unsafe {
        GetComputerNameExW(
            ComputerNamePhysicalDnsHostname,
            name.as_mut_ptr(),
            &raw mut len,
        )
    } != 0
    {
        String::from_utf16_lossy(&name[..len as usize]).into_bytes()
    } else {
        std::env::var("COMPUTERNAME")
            .unwrap_or_else(|_| "localhost".into())
            .into_bytes()
    };
    let ppid = if fake_pid {
        Some(std::process::id())
    } else {
        parent_pid()
    };
    psh_engine::host::HostInfo {
        uid: 1000,
        euid: 1000,
        groups: vec![1000],
        ppid,
        hostname,
        ostype: "msys".into(),
        machtype: format!("{arch}-pc-msys"),
        hosttype: arch,
    }
}

/// The parent's pid from a process snapshot.
fn parent_pid() -> Option<u32> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: a snapshot of all processes; the handle is owned below.
    let snap = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }).ok()?;
    let me = std::process::id();
    // SAFETY: plain data; dwSize set as the API requires.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let h = snap.as_raw_handle() as HANDLE;
    // SAFETY: an open snapshot handle and an initialized entry.
    let mut ok = unsafe { Process32FirstW(h, &raw mut e) } != 0;
    while ok {
        if e.th32ProcessID == me {
            return Some(e.th32ParentProcessID);
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(h, &raw mut e) } != 0;
    }
    None
}

/// The shell's environment from the process's (D1075): Windows names are
/// case-insensitive, shell variables are not, so `Path` (however it is spelled) becomes
/// `PATH`, the name scripts use; `HOME` defaults to `USERPROFILE` and `TMPDIR` to the
/// temp directory, both `/`-separated. For the standalone shell and embedders.
pub fn shell_env(env: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut env: Vec<(Vec<u8>, Vec<u8>)> = env
        .into_iter()
        .map(|(k, v)| {
            if k.eq_ignore_ascii_case(b"PATH") {
                (b"PATH".to_vec(), v)
            } else {
                (k, v)
            }
        })
        .collect();
    let has = |env: &[(Vec<u8>, Vec<u8>)], k: &[u8]| env.iter().any(|(n, _)| n == k);
    if !has(&env, b"HOME")
        && let Some(home) = std::env::var_os("USERPROFILE")
    {
        let home = path::engine_path(std::path::Path::new(&home));
        env.push((b"HOME".to_vec(), home));
    }
    if !has(&env, b"TMPDIR") {
        env.push((b"TMPDIR".to_vec(), path::temp_dir()));
    }
    env
}

#[cfg(test)]
mod tests;
