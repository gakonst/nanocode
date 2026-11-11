//! Spawning, waiting for and abandoning external processes on Windows (ARCH.md "psh-os").
//!
//! **Spawn.** `CreateProcessW` creates the child suspended, it is assigned to a Job
//! Object of its own (no breakaway), then resumed: nothing it starts escapes the job.
//! Only its three stdio handles are inherited (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`), so
//! concurrent spawns never leak pipe ends into each other. Under
//! [`Placement::Group`] it leads a console process group of its own, so a ctrl-break
//! reaches it and what it started. Without a console, children get `CREATE_NO_WINDOW`
//! (no window flashes). The run's jobs kill their processes when the last handle closes,
//! so a psh that dies takes them along; `release` keeps that, detached and disowned
//! externals lose it (D1074).
//!
//! **Programs.** `.exe`/`.com` run as they are; `.bat`/`.cmd` through `cmd.exe` with
//! hardened quoting ([`super::cmdline`]); a file starting with `#!` by its interpreter,
//! looked up on the child's `PATH` (`/usr/bin/env X`, `/bin/X`, `/usr/bin/X` all mean X).
//!
//! **Wait.** A waiter thread per external blocks on its process handle and feeds
//! [`crate::lifecycle::Tracker`]. Holding the handle keeps the pid from being reused.
//!
//! **Signals.** KILL = `TerminateJobObject`. TERM, INT, HUP and QUIT = `CTRL_BREAK_EVENT`
//! to the external's group; when that cannot be delivered (no shared console, or the
//! external shares psh's group), the job is terminated at once. Abandon = TERM, then
//! `TerminateJobObject` when the external exits or the grace period ends: the whole
//! tree dies. Exit codes of crashes become signals (D1074).
//!
//! The recipe follows oh-my-pi's `pi-shell/src/sys/spawn/windows.rs` (MIT, see NOTICE).

use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use psh_engine::HostFile;
use psh_engine::host::{
    ChildFd, ExitStatus, ExtProc, SIGHUP, SIGINT, SIGKILL, SIGQUIT, SIGTERM, Signal, SpawnSpec,
    Spawned,
};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BAD_EXE_FORMAT, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::file::{Decode, WinFile, pump_pipe};
use super::{cmdline, owned, path, resolve_in};
use crate::lifecycle::{Dropped, Owner, Tracker};

/// Where an external lives relative to psh's console process group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Every external leads a console process group of its own (`CREATE_NEW_PROCESS_GROUP`):
    /// TERM can reach it as a ctrl-break. The default (agents, embedding).
    Group,
    /// Externals stay in psh's group, as with cmd: a Ctrl-C typed in the console reaches
    /// them directly. For the standalone shell. Signals then terminate the job.
    Inherit,
}

/// The exit code `TerminateJobObject` gives (128 + KILL, what the shell shows).
const KILL_CODE: u32 = 137;
/// `STATUS_CONTROL_C_EXIT`: ended by a console ctrl event.
const CONTROL_C_EXIT: u32 = 0xC000_013A;
/// `STATUS_DLL_INIT_FAILED`.
const DLL_INIT_FAILED: u32 = 0xC000_0142;

/// Waiter and escalation threads only wait.
const SMALL_STACK: usize = 64 << 10;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

fn raw(h: &impl AsRawHandle) -> HANDLE {
    h.as_raw_handle() as HANDLE
}

/// One Job Object: one external and everything it starts.
pub(crate) struct Job {
    handle: OwnedHandle,
    /// The external's pid (and console group id under `Placement::Group`).
    pid: u32,
    /// It leads a console group of its own that shares psh's console: a ctrl-break can
    /// reach it.
    breakable: bool,
    /// We terminated it: its processes' exit is reported as KILL.
    killed: AtomicBool,
    /// The signal last delivered as a ctrl-break, to name the exit it causes.
    broke: Mutex<Option<Signal>>,
}

impl Job {
    fn new() -> io::Result<Job> {
        // SAFETY: null attributes and name create an anonymous job.
        let handle = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        Ok(Job {
            handle,
            pid: 0,
            breakable: false,
            killed: AtomicBool::new(false),
            broke: Mutex::new(None),
        })
    }

    /// Whether closing the last handle kills the job's processes.
    pub(crate) fn kill_on_close(&self, on: bool) -> io::Result<()> {
        // SAFETY: plain data; zeroed means no limits.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        if on {
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        }
        // SAFETY: our job handle; `info` is valid for its size.
        let ok = unsafe {
            SetInformationJobObject(
                raw(&self.handle),
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(crate) fn terminate(&self) {
        self.killed.store(true, Ordering::Release);
        // SAFETY: our job handle.
        unsafe { TerminateJobObject(raw(&self.handle), KILL_CODE) };
    }

    /// A ctrl-break for `sig` to the job's group; false when it cannot be delivered.
    fn ctrl_break(&self, sig: Signal) -> bool {
        if !self.breakable {
            return false;
        }
        *lock(&self.broke) = Some(sig);
        // SAFETY: plain call with scalar arguments.
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.pid) != 0 }
    }

    /// Processes left in the job.
    pub(crate) fn active(&self) -> u32 {
        // SAFETY: plain data, filled by the query.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: our job handle; `info` is writable for its size.
        let ok = unsafe {
            QueryInformationJobObject(
                raw(&self.handle),
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                std::mem::size_of_val(&info) as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { 0 } else { info.ActiveProcesses }
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Delivers `sig` to the whole job: KILL terminates it; TERM, INT, HUP and QUIT are
    /// a ctrl-break, or termination where that is impossible; 0 checks that it lives.
    pub(crate) fn signal(&self, sig: Signal) -> io::Result<()> {
        match sig {
            0 => {
                if self.active() > 0 {
                    Ok(())
                } else {
                    Err(io::Error::from_raw_os_error(87))
                }
            }
            SIGKILL => {
                self.terminate();
                Ok(())
            }
            SIGTERM | SIGINT | SIGHUP | SIGQUIT => {
                if !self.ctrl_break(sig) {
                    self.terminate();
                }
                Ok(())
            }
            _ => Err(io::ErrorKind::Unsupported.into()),
        }
    }

    /// The status of the external's exit `code`.
    fn status(&self, code: u32) -> ExitStatus {
        if self.killed.load(Ordering::Acquire) && code == KILL_CODE {
            return ExitStatus::Signaled(SIGKILL);
        }
        exit_status(code, *lock(&self.broke))
    }
}

/// An exit code as the shell reports it (D1074): a console break as the signal that
/// caused it, crash codes as the signal a Unix crash would give, other codes as exits.
pub(crate) fn exit_status(code: u32, broke: Option<Signal>) -> ExitStatus {
    match code {
        CONTROL_C_EXIT => ExitStatus::Signaled(broke.unwrap_or(SIGINT)),
        // A break that arrives while the process still initializes fails its start.
        DLL_INIT_FAILED if broke.is_some() => ExitStatus::Signaled(broke.unwrap_or(SIGINT)),
        // Access violation, stack overflow, in-page error.
        0xC000_0005 | 0xC000_00FD | 0xC000_0006 => ExitStatus::Signaled(11),
        // Illegal or privileged instruction.
        0xC000_001D | 0xC000_0096 => ExitStatus::Signaled(4),
        // Integer and floating point faults.
        0xC000_008E..=0xC000_0095 => ExitStatus::Signaled(8),
        // Fail-fast (`abort`, panic=abort) and assertion failures.
        0xC000_0409 | 0xC000_0420 => ExitStatus::Signaled(6),
        c => ExitStatus::Exited(c as i32),
    }
}

/// Spawner state shared by a host and the processes it spawned.
pub(crate) struct Shared {
    pub(crate) grace: Duration,
    pub(crate) placement: Placement,
    /// psh has a console its children can share (else they get a hidden one each).
    pub(crate) console: bool,
    pub(crate) decode: Decode,
    /// `WindowsConfig::detached_survive`.
    pub(crate) detached_survive: bool,
    /// The run's jobs (all but detached and disowned externals): leak checks and the
    /// leftovers killed at the end.
    pub(crate) jobs: Mutex<Vec<Arc<Job>>>,
}

impl Shared {
    fn record(&self, job: &Arc<Job>) {
        let mut jobs = lock(&self.jobs);
        if jobs.len() >= 256 {
            jobs.retain(|j| Arc::strong_count(j) > 1 || j.active() > 0);
        }
        jobs.push(job.clone());
    }

    fn unrecord(&self, job: &Arc<Job>) {
        lock(&self.jobs).retain(|j| !Arc::ptr_eq(j, job));
    }

    /// The recorded job of `pid` (an external of the run).
    pub(crate) fn job_of(&self, pid: u32) -> Option<Arc<Job>> {
        lock(&self.jobs).iter().find(|j| j.pid == pid).cloned()
    }

    /// Kills what is left of the run: a ctrl-break to every job with live processes
    /// now, termination of those still alive after the grace period (on a thread,
    /// counted for [`settle`]); jobs that cannot get a break are terminated at once.
    pub(crate) fn kill_all(&self) {
        let live: Vec<Arc<Job>> = lock(&self.jobs)
            .iter()
            .filter(|j| j.active() > 0)
            .cloned()
            .collect();
        let mut waiting = Vec::new();
        for j in live {
            if j.ctrl_break(SIGTERM) {
                waiting.push(j);
            } else {
                j.terminate();
            }
        }
        if waiting.is_empty() {
            return;
        }
        let grace = self.grace;
        *lock(&ESCALATING) += 1;
        let fallback = waiting.clone();
        let started = std::thread::Builder::new()
            .name("psh-kill".into())
            .stack_size(SMALL_STACK)
            .spawn(move || {
                let deadline = Instant::now() + grace;
                while Instant::now() < deadline && waiting.iter().any(|j| j.active() > 0) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                for j in &waiting {
                    if j.active() > 0 {
                        j.terminate();
                    }
                }
                *lock(&ESCALATING) -= 1;
                SETTLED.notify_all();
            });
        if started.is_err() {
            *lock(&ESCALATING) -= 1;
            for j in &fallback {
                j.terminate();
            }
        }
    }

    /// The run is over and its leftovers stay: they must not die when the jobs close.
    pub(crate) fn keep_all(&self) {
        for j in lock(&self.jobs).iter() {
            let _ = j.kill_on_close(false);
        }
    }
}

/// Escalations running (abandoned externals not yet killed), process-wide.
static ESCALATING: Mutex<usize> = Mutex::new(0);
static SETTLED: Condvar = Condvar::new();

/// Waits up to `timeout` until every abandoned external of the process was killed;
/// whether they were. A standalone shell calls it before it exits.
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

/// What to start for a program: the application and its command line.
struct Command {
    app: Vec<u16>,
    line: Vec<u16>,
}

/// The system directory (`C:\Windows\System32`).
pub(crate) fn system_dir() -> Option<std::path::PathBuf> {
    let mut buf = vec![0u16; 260];
    // SAFETY: `buf` is writable for its length.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(std::path::PathBuf::from(String::from_utf16_lossy(
        &buf[..n],
    )))
}

/// `%SystemRoot%\System32\cmd.exe`.
fn system_cmd() -> io::Result<String> {
    let mut buf = vec![0u16; 260];
    // SAFETY: `buf` is writable for its length.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(format!("{}\\cmd.exe", String::from_utf16_lossy(&buf[..n])))
}

fn text(b: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(b).map_err(|_| invalid("argument is not valid UTF-8".into()))
}

fn extension(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The `#!` line of `p`: the interpreter and the rest of the line.
pub(crate) fn shebang(p: &Path) -> Option<(String, String)> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut head = [0u8; 256];
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return None,
        }
    }
    let head = &head[..n];
    let line = head.strip_prefix(b"#!")?;
    let end = line.iter().position(|&b| b == b'\n').unwrap_or(line.len());
    let line = String::from_utf8_lossy(&line[..end]);
    let line = line.trim_end_matches('\r').trim();
    let (interp, rest) = match line.find([' ', '\t']) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    };
    (!interp.is_empty()).then(|| (interp.to_string(), rest.to_string()))
}

/// The answer for a script psh runs itself (D1079, D1300): `#!/bin/sh`, `#!/bin/bash`
/// (also through `/usr/bin/env`), and a file with neither an interpreter line nor an
/// executable extension; the engine runs it as a psh script in a subshell.
fn run_in_shell(_p: &Path) -> io::Error {
    psh_engine::host::RunInShell::error()
}

/// The error of a `#!` script whose interpreter is on no `PATH` entry (`NotFound`):
/// the name, for a test runner that counts missing programs.
#[derive(Debug)]
pub struct MissingInterpreter(pub String);

impl std::fmt::Display for MissingInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: interpreter not found", self.0)
    }
}

impl std::error::Error for MissingInterpreter {}

/// The child's `PATH` (`Path` and any other case).
fn env_path(env: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    env.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(b"PATH"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// The application and command line for `program` with `argv`, following batch files
/// and `#!` lines (one level of interpreter).
fn command(
    program: &Path,
    argv: &[&str],
    env: &[(Vec<u8>, Vec<u8>)],
    cwd: &[u8],
    depth: u32,
) -> io::Result<Command> {
    let app_str = program.to_string_lossy().into_owned();
    let ext = extension(program);
    // A missing program is "not found" (127), before any guess about what it is.
    if depth == 0
        && let Err(e) = std::fs::metadata(program)
    {
        return Err(e);
    }
    if cmdline::is_batch(&app_str) {
        let line = cmdline::batch_command_line(&app_str, argv.get(1..).unwrap_or_default())?;
        let app = path::wide(OsStr::new(&system_cmd()?))?;
        return Ok(Command { app, line });
    }
    if ext != "exe" && ext != "com" && depth == 0 {
        match shebang(program) {
            Some((interp, rest)) => {
                let mut words: Vec<&str> = rest.split_whitespace().collect();
                let mut name = interp.rsplit(['/', '\\']).next().unwrap_or(&interp);
                if name == "env" {
                    // `#!/usr/bin/env [-S] X ARGS`
                    if words.first() == Some(&"-S") {
                        words.remove(0);
                    }
                    if words.is_empty() {
                        return Err(run_in_shell(program));
                    }
                    name = words.remove(0);
                }
                if matches!(name, "sh" | "bash") {
                    return Err(run_in_shell(program));
                }
                // An interpreter written as a Windows path is used as it is.
                let direct = Path::new(&interp);
                let interp_path = if direct.is_absolute() && direct.is_file() && name != "env" {
                    direct.to_path_buf()
                } else {
                    let path_var = env_path(env);
                    // Windows' Python installs `python.exe` (and `py.exe`), not
                    // `python3.exe`, which `#!/usr/bin/env python3` asks for (D1076).
                    let found = resolve_in(name.as_bytes(), &path_var, cwd)
                        .or_else(|| {
                            (name == "python3")
                                .then(|| resolve_in(b"python", &path_var, cwd))
                                .flatten()
                        })
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::NotFound,
                                MissingInterpreter(name.to_string()),
                            )
                        })?;
                    path::native(&found)?
                };
                let mut args: Vec<&str> = vec![name];
                args.extend(words);
                args.push(&app_str);
                args.extend(argv.iter().skip(1));
                return command(&interp_path, &args, env, cwd, depth + 1);
            }
            None if !ext.is_empty() && super::pathext().iter().any(|e| e[1..] == *ext) => {}
            None => return Err(run_in_shell(program)),
        }
    }
    let line = cmdline::command_line(argv)?;
    Ok(Command {
        app: path::wide(program.as_os_str())?,
        line,
    })
}

/// `CREATE_UNICODE_ENVIRONMENT` block: `K=V\0...\0`, sorted by name without case as
/// Windows requires; of names equal without case, the last wins.
pub(crate) fn env_block(env: &[(Vec<u8>, Vec<u8>)]) -> io::Result<Vec<u16>> {
    let mut vars: Vec<(String, &str, &str)> = Vec::with_capacity(env.len());
    for (k, v) in env {
        let k = text(k)?;
        let v = text(v)?;
        // Names may start with `=` (the per-drive `=C:` variables).
        if k.is_empty() || k.contains('\0') || v.contains('\0') || k[1..].contains('=') {
            return Err(invalid(format!("invalid environment variable name: {k}")));
        }
        vars.push((k.to_uppercase(), k, v));
    }
    // Stable: equal names keep their order, so the last one is kept below.
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    let mut block = Vec::new();
    for (i, (upper, k, v)) in vars.iter().enumerate() {
        if vars.get(i + 1).is_some_and(|next| next.0 == *upper) {
            continue;
        }
        block.extend(k.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(v.encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// An inheritable duplicate of `h`, closed after the spawn.
fn inheritable(h: RawHandle) -> io::Result<OwnedHandle> {
    let mut dup: HANDLE = std::ptr::null_mut();
    // SAFETY: `h` is a handle the engine keeps open during the spawn; the duplicate is
    // ours.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            h as HANDLE,
            GetCurrentProcess(),
            &raw mut dup,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle we own.
    Ok(unsafe { OwnedHandle::from_raw_handle(dup as RawHandle) })
}

/// A process attribute list holding the handles to inherit.
struct Attributes {
    buf: Vec<usize>,
}

impl Attributes {
    fn with_handles(handles: &[HANDLE]) -> io::Result<Attributes> {
        let mut size = 0usize;
        // SAFETY: a null list with a size pointer is the documented size query.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &raw mut size) };
        let mut buf = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: `buf` holds at least `size` bytes, aligned.
        if unsafe {
            InitializeProcThreadAttributeList(buf.as_mut_ptr().cast(), 1, 0, &raw mut size)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut a = Attributes { buf };
        // SAFETY: the list is initialized; `handles` outlives CreateProcessW (the caller
        // keeps it alive across the call).
        let ok = unsafe {
            UpdateProcThreadAttribute(
                a.list(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(a)
    }

    fn list(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buf.as_mut_ptr().cast()
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized in `with_handles` (a failed initialization leaves a list
        // that deleting does not touch beyond its buffer).
        unsafe { DeleteProcThreadAttributeList(self.list()) };
    }
}

pub(crate) fn spawn(shared: &Arc<Shared>, mut spec: SpawnSpec) -> io::Result<Spawned> {
    let program = path::native(&spec.program)?;
    let argv: Vec<&str> = spec
        .argv
        .iter()
        .map(|a| text(a))
        .collect::<io::Result<_>>()?;
    let argv = if argv.is_empty() {
        vec![text(&spec.program)?]
    } else {
        argv
    };
    let mut cmd = command(&program, &argv, &spec.env, &spec.cwd, 0)?;
    let env = env_block(&spec.env)?;
    let cwd = path::wide(path::native(&spec.cwd)?.as_os_str())?;

    // The child's stdio: inheritable handles of our own, closed after the spawn.
    let mut handles: Vec<(u32, OwnedHandle)> = Vec::new();
    let mut pipes: Vec<(u32, Rc<dyn HostFile>)> = Vec::new();
    let mut held: Vec<Rc<dyn HostFile>> = Vec::new();
    for (n, fd) in std::mem::take(&mut spec.fds) {
        // Native programs have no descriptors beyond 0-2 (D1074).
        if n > 2 {
            continue;
        }
        let h = match fd {
            ChildFd::File(f) => {
                let any: &dyn std::any::Any = &*f;
                let raw_h = any
                    .downcast_ref::<WinFile>()
                    .and_then(WinFile::raw_handle)
                    .ok_or_else(|| io::Error::other("not a file this host can pass on"))?;
                let dup = inheritable(raw_h)?;
                held.push(f);
                dup
            }
            ChildFd::Pipe { input } => {
                let (ours, theirs) = pump_pipe(input, shared.decode)?;
                pipes.push((n, Rc::new(ours) as Rc<dyn HostFile>));
                theirs
            }
            ChildFd::Dup(m) => {
                let Some((_, h)) = handles.iter().find(|(t, _)| *t == m) else {
                    return Err(invalid("dup of a descriptor not given".into()));
                };
                inheritable(h.as_raw_handle())?
            }
            ChildFd::Null => {
                let f = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("NUL")?;
                inheritable(f.as_raw_handle())?
            }
        };
        handles.retain(|(t, _)| *t != n);
        handles.push((n, h));
    }
    let std_handle = |n: u32| -> io::Result<OwnedHandle> {
        match handles.iter().find(|(t, _)| *t == n) {
            Some((_, h)) => inheritable(h.as_raw_handle()),
            None => {
                let f = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("NUL")?;
                inheritable(f.as_raw_handle())
            }
        }
    };
    let stdio = [std_handle(0)?, std_handle(1)?, std_handle(2)?];
    drop(handles);

    let mut job = Job::new()?;
    let group = spec.detach || shared.placement == Placement::Group;
    job.breakable = group && shared.console;
    // Part of the run: killed with its leftovers, and when psh dies.
    let in_run = !spec.detach || !shared.detached_survive;
    if in_run {
        job.kill_on_close(true)?;
    }
    let (process, pid) = create(&mut cmd, &env, &cwd, &stdio, &job, group, shared.console)?;
    // Our copies of the child's ends: the pumps see end of file when it closes its own.
    drop(stdio);
    drop(held);
    job.pid = pid;
    let job = Arc::new(job);
    if in_run {
        shared.record(&job);
    }
    let tracker = Arc::new(Tracker::new());
    let process = Arc::new(process);
    if let Err(e) = watch(process.clone(), job.clone(), tracker.clone()) {
        job.terminate();
        shared.unrecord(&job);
        return Err(e);
    }
    Ok(Spawned {
        proc: Box::new(WinProc {
            pid,
            job,
            tracker,
            shared: shared.clone(),
            detached: spec.detach,
            _process: process,
        }),
        pipes,
    })
}

/// `CreateProcessW` suspended, into `job`, then resumed.
fn create(
    cmd: &mut Command,
    env: &[u16],
    cwd: &[u16],
    stdio: &[OwnedHandle; 3],
    job: &Job,
    group: bool,
    console: bool,
) -> io::Result<(OwnedHandle, u32)> {
    let mut list: Vec<HANDLE> = Vec::with_capacity(3);
    for h in stdio {
        if !list.contains(&raw(h)) {
            list.push(raw(h));
        }
    }
    let mut attrs = Attributes::with_handles(&list)?;
    // SAFETY: plain data; zeroed is a valid empty startup info.
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = raw(&stdio[0]);
    si.StartupInfo.hStdOutput = raw(&stdio[1]);
    si.StartupInfo.hStdError = raw(&stdio[2]);
    si.lpAttributeList = attrs.list();
    // SAFETY: plain data, filled by CreateProcessW.
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let mut flags = CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT;
    if group {
        flags |= CREATE_NEW_PROCESS_GROUP;
    }
    if !console {
        flags |= CREATE_NO_WINDOW;
    }
    cmd.line.push(0);
    // SAFETY: every pointer is valid for the call: NUL-terminated wide strings, a
    // writable command line, the environment block, the startup info with its attribute
    // list and handle list, which outlive the call.
    let ok = unsafe {
        CreateProcessW(
            cmd.app.as_ptr(),
            cmd.line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            flags,
            env.as_ptr().cast(),
            cwd.as_ptr(),
            (&raw const si).cast::<STARTUPINFOW>(),
            &raw mut pi,
        )
    };
    if ok == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_BAD_EXE_FORMAT as i32) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cannot execute binary file",
            ));
        }
        return Err(e);
    }
    drop(attrs);
    // SAFETY: fresh handles CreateProcessW returned to us.
    let (process, thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(pi.hProcess as RawHandle),
            OwnedHandle::from_raw_handle(pi.hThread as RawHandle),
        )
    };
    // SAFETY: open job and process handles.
    if unsafe { AssignProcessToJobObject(raw(&job.handle), raw(&process)) } == 0 {
        let e = io::Error::last_os_error();
        // SAFETY: the suspended child never ran.
        unsafe { TerminateProcess(raw(&process), 1) };
        return Err(e);
    }
    // SAFETY: the child's primary thread, created suspended.
    if unsafe { ResumeThread(raw(&thread)) } == u32::MAX {
        let e = io::Error::last_os_error();
        // SAFETY: as above.
        unsafe { TerminateProcess(raw(&process), 1) };
        return Err(e);
    }
    Ok((process, pi.dwProcessId))
}

/// A waiter thread: blocks on the process handle, then reports the exit.
fn watch(process: Arc<OwnedHandle>, job: Arc<Job>, tracker: Arc<Tracker>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("psh-wait".into())
        .stack_size(SMALL_STACK)
        .spawn(move || {
            // SAFETY: an open process handle, kept alive by `process`.
            let r = unsafe { WaitForSingleObject(raw(&*process), INFINITE) };
            let status = if r == WAIT_OBJECT_0 {
                let mut code = 0u32;
                // SAFETY: an exited process handle; `code` is writable.
                if unsafe { GetExitCodeProcess(raw(&*process), &raw mut code) } != 0 {
                    job.status(code)
                } else {
                    ExitStatus::Exited(127)
                }
            } else {
                ExitStatus::Exited(127)
            };
            // Nothing to reap on Windows: the handle closes when the last owner drops.
            if let Some(w) = tracker.exited(status, || {}) {
                w.wake();
            }
        })
        .map(drop)
}

/// A spawned external in its job.
struct WinProc {
    pid: u32,
    job: Arc<Job>,
    tracker: Arc<Tracker>,
    shared: Arc<Shared>,
    /// `SpawnSpec::detach`: dropping it releases instead of abandoning.
    detached: bool,
    /// Held so the pid is not reused while this exists.
    _process: Arc<OwnedHandle>,
}

impl ExtProc for WinProc {
    fn pid(&self) -> Option<u32> {
        Some(self.pid)
    }

    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<ExitStatus> {
        self.tracker.poll(cx)
    }

    fn signal(&mut self, sig: Signal) {
        let mut life = self.tracker.lock();
        if sig == SIGTERM {
            life.term_sent();
        }
        drop(life);
        let _ = self.job.signal(sig);
    }

    fn release(self: Box<Self>) {
        // Still part of the run: its job is killed with the leftovers.
        let mut life = self.tracker.lock();
        if Tracker::release(&mut life) {
            life.set_reaped();
        }
    }

    fn disown(self: Box<Self>) {
        // No longer the run's: not killed with the leftovers, nor when psh dies.
        self.shared.unrecord(&self.job);
        let _ = self.job.kill_on_close(false);
        self.release();
    }
}

impl Drop for WinProc {
    fn drop(&mut self) {
        let mut life = self.tracker.lock();
        if life.owner() != Owner::Engine {
            return;
        }
        if self.detached {
            if Tracker::release(&mut life) {
                life.set_reaped();
            }
            return;
        }
        match Tracker::abandon(&mut life) {
            Dropped::Reap => life.set_reaped(),
            Dropped::Escalate { term } => {
                drop(life);
                if term && !self.job.ctrl_break(SIGTERM) {
                    self.job.terminate();
                }
                let deadline = Instant::now() + self.shared.grace;
                let (tracker, job) = (self.tracker.clone(), self.job.clone());
                *lock(&ESCALATING) += 1;
                let started = std::thread::Builder::new()
                    .name("psh-kill".into())
                    .stack_size(SMALL_STACK)
                    .spawn(move || {
                        // Terminate the job when the external exits or the grace period
                        // ends: what it started dies with it.
                        tracker.wait_exit_until(Some(deadline));
                        job.terminate();
                        tracker.wait_exit_until(None);
                        tracker.lock().set_reaped();
                        *lock(&ESCALATING) -= 1;
                        SETTLED.notify_all();
                    });
                if started.is_err() {
                    *lock(&ESCALATING) -= 1;
                    self.job.terminate();
                    let mut life = self.tracker.lock();
                    if Tracker::release(&mut life) {
                        life.set_reaped();
                    }
                }
            }
        }
    }
}

/// Whether psh can share a console with its children (it has one).
pub(crate) fn has_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    // SAFETY: no arguments.
    !unsafe { GetConsoleWindow() }.is_null()
}

/// Terminates process `pid` that is not one of the run's externals (`kill PID`).
pub(crate) fn kill_pid(pid: u32, sig: Signal) -> io::Result<()> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    let access = if sig == 0 {
        PROCESS_QUERY_LIMITED_INFORMATION
    } else {
        PROCESS_TERMINATE
    };
    // SAFETY: plain call with scalar arguments.
    let h = owned(unsafe { OpenProcess(access, 0, pid) })?;
    match sig {
        0 => Ok(()),
        SIGKILL | SIGTERM | SIGINT | SIGHUP | SIGQUIT => {
            // SAFETY: an open handle with PROCESS_TERMINATE.
            if unsafe { TerminateProcess(raw(&h), 128 + sig as u32) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        _ => Err(io::ErrorKind::Unsupported.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_block_sorts_without_case_and_keeps_the_last() {
        let env = vec![
            (b"b".to_vec(), b"1".to_vec()),
            (b"Path".to_vec(), b"x".to_vec()),
            (b"A".to_vec(), b"2".to_vec()),
            (b"PATH".to_vec(), b"y".to_vec()),
        ];
        let block = env_block(&env).unwrap();
        let s = String::from_utf16(&block).unwrap();
        assert_eq!(s, "A=2\0b=1\0PATH=y\0\0");
        assert_eq!(env_block(&[]).unwrap(), [0, 0]);
        assert!(env_block(&[(b"a=b".to_vec(), b"c".to_vec())]).is_err());
    }

    #[test]
    fn exit_codes_of_crashes_are_signals() {
        assert_eq!(exit_status(0, None), ExitStatus::Exited(0));
        assert_eq!(exit_status(3, None), ExitStatus::Exited(3));
        assert_eq!(exit_status(0xC000_0005, None), ExitStatus::Signaled(11));
        assert_eq!(exit_status(0xC000_0409, None), ExitStatus::Signaled(6));
        assert_eq!(
            exit_status(CONTROL_C_EXIT, None),
            ExitStatus::Signaled(SIGINT)
        );
        assert_eq!(
            exit_status(CONTROL_C_EXIT, Some(SIGTERM)),
            ExitStatus::Signaled(SIGTERM)
        );
    }

    #[test]
    fn shebang_lines() {
        let dir = std::env::temp_dir().join(format!("psh-shebang-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s");
        std::fs::write(&f, "#!/usr/bin/env node --x\r\nrest").unwrap();
        assert_eq!(
            shebang(&f),
            Some(("/usr/bin/env".into(), "node --x".into()))
        );
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        assert_eq!(shebang(&f), Some(("/bin/sh".into(), String::new())));
        std::fs::write(&f, "echo hi\n").unwrap();
        assert_eq!(shebang(&f), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
