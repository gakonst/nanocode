use std::{
    env,
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(unix)]
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use tokio::process::{ChildStderr, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use nanocode_process_contracts::{MIN_REDACTED_SECRET_BYTES, REDACTED, Redactor};

use super::selection::Shell;

const SENSITIVE_ENV_PARTS: [&str; 11] = [
    "AUTH",
    "AUTHORIZATION",
    "COOKIE",
    "CREDENTIAL",
    "CREDENTIALS",
    "KEY",
    "PASS",
    "PASSWD",
    "PASSWORD",
    "SECRET",
    "TOKEN",
];

const NORMALIZED_ENVIRONMENT: [(&str, &str); 10] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("LANG", "C.UTF-8"),
    ("LC_CTYPE", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
    ("COLORTERM", ""),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("CODEX_CI", "1"),
];

pub(super) struct SpawnedProcess {
    pub(super) child: ProcessChild,
    pub(super) stdin: Option<ProcessStdin>,
    pub(super) output: ProcessOutput,
    pub(super) process_group: SharedProcessGroup,
}

pub(super) struct ProcessChild {
    reaping: Reaping,
    process_group: SharedProcessGroup,
    exited: Arc<Exit>,
}

#[derive(Default)]
pub(super) struct Exit {
    done: AtomicBool,
    waiters: std::sync::OnceLock<Arc<tokio::sync::Notify>>,
}

impl Exit {
    fn mark(&self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(waiters) = self.waiters.get() {
            waiters.notify_waiters();
        }
    }

    pub(super) fn get(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    pub(super) fn notify(&self, waiters: &Arc<tokio::sync::Notify>) {
        let _ = self.waiters.set(Arc::clone(waiters));
    }
}

enum Reaping {
    Waiting(JoinHandle<io::Result<i32>>),
    Reaped(io::Result<i32>),
}

impl ProcessChild {
    pub(super) fn new(
        mut try_wait: impl FnMut() -> io::Result<Option<i32>> + Send + 'static,
        pid: u32,
        io_driver_available: bool,
    ) -> Self {
        #[cfg(not(unix))]
        let _ = io_driver_available;
        let (commands, mut received) = mpsc::unbounded_channel();
        let process_group = SharedProcessGroup(commands);
        let mut guard = ProcessGroupGuard::new(pid);
        let exited = Arc::new(Exit::default());
        let reaped = Arc::clone(&exited);
        let wait = tokio::spawn(async move {
            let mut listening = true;
            #[cfg(unix)]
            let mut exited = if io_driver_available {
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child()) {
                    Ok(signal) => Some(signal),
                    Err(error) => {
                        tracing::warn!(%error, "shell exit notifications unavailable; using polling");
                        None
                    }
                }
            } else {
                None
            };
            loop {
                while let Ok(command) = received.try_recv() {
                    command.apply(&mut guard);
                }
                let ready = guard.prepare_to_reap();
                match ready.and_then(|ready| if ready { try_wait() } else { Ok(None) }) {
                    Ok(None) => {}
                    Ok(Some(status)) => {
                        guard.disarm();
                        reaped.mark();
                        return Ok(status);
                    }

                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        guard.disarm();
                        reaped.mark();
                        return Err(error);
                    }
                }
                let exit = async {
                    #[cfg(unix)]
                    if let Some(signal) = exited.as_mut() {
                        return signal.recv().await.is_some();
                    }
                    tokio::time::sleep(REAP_POLL_INTERVAL).await;
                    true
                };
                tokio::select! {
                    command = received.recv(), if listening => match command {
                        Some(command) => command.apply(&mut guard),
                        None => listening = false,
                    },
                    notified = exit => {
                        #[cfg(unix)]
                        if !notified {
                            exited = None;
                        }
                        #[cfg(not(unix))]
                        let _ = notified;
                    }
                }
            }
        });
        Self {
            reaping: Reaping::Waiting(wait),
            process_group,
            exited,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn served(
        mut guard: ProcessGroupGuard,
        mut exit: oneshot::Receiver<io::Result<i32>>,
    ) -> Self {
        let (commands, mut received) = mpsc::unbounded_channel();
        let process_group = SharedProcessGroup(commands);
        let exited = Arc::new(Exit::default());
        let reaped = Arc::clone(&exited);
        let wait = tokio::spawn(async move {
            let mut listening = true;
            let result = loop {
                tokio::select! {
                    command = received.recv(), if listening => match command {
                        Some(command) => command.apply(&mut guard),
                        None => listening = false,
                    },
                    status = &mut exit => {
                        let status = status.unwrap_or_else(|_| Err(io::Error::other("the shell server stopped")));
                        if status.is_ok() {
                            let _ = guard.terminate_and_disarm();
                        }
                        break status;
                    }
                }
            };
            reaped.mark();
            result
        });
        Self {
            reaping: Reaping::Waiting(wait),
            process_group,
            exited,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn process_group(&self) -> SharedProcessGroup {
        self.process_group.clone()
    }

    pub(super) fn exited(&self) -> Arc<Exit> {
        Arc::clone(&self.exited)
    }

    pub(super) async fn wait(&mut self) -> io::Result<i32> {
        fn copy(result: &io::Result<i32>) -> io::Result<i32> {
            match result {
                Ok(status) => Ok(*status),
                Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
            }
        }
        let result = match &mut self.reaping {
            Reaping::Reaped(result) => return copy(result),
            Reaping::Waiting(wait) => wait
                .await
                .map_err(|error| io::Error::other(format!("shell wait task failed: {error}")))
                .and_then(|result| result),
        };
        let reply = copy(&result);
        self.reaping = Reaping::Reaped(result);
        reply
    }
}

impl Drop for ProcessChild {
    fn drop(&mut self) {
        self.process_group.terminate_and_disarm();
    }
}

pub(super) const REAP_POLL_INTERVAL: Duration = Duration::from_millis(10);

type Reply = oneshot::Sender<io::Result<()>>;

enum GroupCommand {
    Interrupt(Reply),
    Terminate(Option<Reply>),
}

impl GroupCommand {
    fn apply(self, guard: &mut ProcessGroupGuard) {
        match self {
            Self::Interrupt(reply) => drop(reply.send(guard.interrupt())),
            Self::Terminate(reply) => {
                let result = guard.terminate_and_disarm();
                if let Some(reply) = reply {
                    drop(reply.send(result));
                }
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct SharedProcessGroup(mpsc::UnboundedSender<GroupCommand>);

impl SharedProcessGroup {
    async fn request(&self, command: impl FnOnce(Reply) -> GroupCommand) -> io::Result<()> {
        let (reply, response) = oneshot::channel();
        if self.0.send(command(reply)).is_err() {
            return Ok(());
        }
        response.await.unwrap_or(Ok(()))
    }

    pub(super) async fn interrupt(&self) -> io::Result<()> {
        self.request(GroupCommand::Interrupt).await
    }

    pub(super) async fn terminate(&self) -> io::Result<()> {
        self.request(|reply| GroupCommand::Terminate(Some(reply)))
            .await
    }

    pub(super) fn terminate_and_disarm(&self) {
        let _ = self.0.send(GroupCommand::Terminate(None));
    }
}

type PtyWrite = (Vec<u8>, oneshot::Sender<io::Result<()>>);

pub(super) enum ProcessStdin {
    Pty(mpsc::UnboundedSender<PtyWrite>),
}

impl ProcessStdin {
    #[cfg(unix)]
    fn pty(terminal: std::os::fd::OwnedFd) -> Self {
        let mut writer = std::fs::File::from(terminal);
        let (sender, mut writes) = mpsc::unbounded_channel::<PtyWrite>();
        drop(tokio::task::spawn_blocking(move || {
            while let Some((bytes, reply)) = writes.blocking_recv() {
                drop(reply.send(writer.write_all(&bytes).and_then(|()| writer.flush())));
            }
            end_input(&mut writer);
        }));
        Self::Pty(sender)
    }

    pub(super) async fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Pty(writer) => {
                let stopped = || io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped");
                let (reply, written) = oneshot::channel();
                writer
                    .send((bytes.to_vec(), reply))
                    .map_err(|_| stopped())?;
                written.await.map_err(|_| stopped())?
            }
        }
    }
}

pub(super) enum ProcessOutput {
    Pipes {
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
    },
    Pty(Box<dyn Read + Send>),
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Streams {
        stdout: super::embedded::Reader,
        stderr: super::embedded::Reader,
    },
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Stream(super::embedded::Reader),
}

pub(super) fn spawn(
    script: &str,
    workspace: &Path,
    shell: &Shell,
    login: bool,
    tty: bool,
    environment: &[(OsString, OsString)],
) -> io::Result<SpawnedProcess> {
    if tty {
        #[cfg(unix)]
        return spawn_pty(script, workspace, shell, login, environment);
        #[cfg(not(unix))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a terminal session (tty) needs a Unix host",
        ));
    }

    spawn_pipes(script, workspace, shell, login, environment)
}

fn spawn_pipes(
    script: &str,
    workspace: &Path,
    shell: &Shell,
    login: bool,
    environment: &[(OsString, OsString)],
) -> io::Result<SpawnedProcess> {
    let mut command = Command::new(shell.path());
    command
        .args(shell.args(script, login))
        .current_dir(workspace)
        .env_clear()
        .envs(environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawned shell without a process identifier"))?;
    let output = ProcessOutput::Pipes {
        stdout: child.stdout.take(),
        stderr: child.stderr.take(),
    };
    let child = ProcessChild::new(
        move || child.try_wait().map(|status| status.map(exit_code)),
        pid,
        true,
    );
    Ok(SpawnedProcess {
        stdin: None,
        output,
        process_group: child.process_group.clone(),
        child,
    })
}

#[cfg(unix)]
fn spawn_pty(
    script: &str,
    workspace: &Path,
    shell: &Shell,
    login: bool,
    environment: &[(OsString, OsString)],
) -> io::Result<SpawnedProcess> {
    let (controller, terminal) = open_terminal()?;
    rustix::termios::tcsetwinsize(
        &terminal,
        rustix::termios::Winsize {
            ws_row: TERMINAL_ROWS,
            ws_col: TERMINAL_COLUMNS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )?;
    let mut command = std::process::Command::new(shell.path());
    command
        .args(shell.args(script, login))
        .current_dir(workspace)
        .env_clear()
        .envs(environment.iter().cloned())
        .stdin(Stdio::from(terminal.try_clone()?))
        .stdout(Stdio::from(terminal.try_clone()?))
        .stderr(Stdio::from(terminal));
    lead_session(&mut command);
    let mut child = command.spawn()?;
    drop(command);
    let pid = child.id();
    let reader = std::fs::File::from(controller.try_clone()?);
    let child = ProcessChild::new(
        move || {
            child
                .try_wait()
                .map(|status| status.map(|status| status.code().unwrap_or(1)))
        },
        pid,
        false,
    );

    Ok(SpawnedProcess {
        process_group: child.process_group.clone(),
        child,
        stdin: Some(ProcessStdin::pty(controller)),
        output: ProcessOutput::Pty(Box::new(reader)),
    })
}

#[cfg(unix)]
const TERMINAL_ROWS: u16 = 24;
#[cfg(unix)]
const TERMINAL_COLUMNS: u16 = 80;

#[cfg(unix)]
const SESSION_DEFAULT_SIGNALS: [Signal; 6] = [
    Signal::SIGCHLD,
    Signal::SIGHUP,
    Signal::SIGINT,
    Signal::SIGQUIT,
    Signal::SIGTERM,
    Signal::SIGALRM,
];

#[cfg(unix)]
fn open_terminal() -> io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use rustix::fs::{Mode, OFlags, open};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    #[cfg(target_os = "linux")]
    let controller = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
    #[cfg(not(target_os = "linux"))]
    let controller = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    #[cfg(not(target_os = "linux"))]
    rustix::io::fcntl_setfd(&controller, rustix::io::FdFlags::CLOEXEC)?;
    grantpt(&controller)?;
    unlockpt(&controller)?;
    let terminal = open(
        ptsname(&controller, Vec::new())?.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok((controller, terminal))
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn lead_session(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    let lead = || -> io::Result<()> {
        for signal in SESSION_DEFAULT_SIGNALS {
            unsafe { nix::sys::signal::signal(signal, nix::sys::signal::SigHandler::SigDfl) }?;
        }
        rustix::process::setsid()?;
        rustix::process::ioctl_tiocsctty(rustix::stdio::stdin())?;
        Ok(())
    };
    unsafe { command.pre_exec(lead) };
}

#[cfg(unix)]
fn end_input(writer: &mut std::fs::File) {
    let Ok(settings) = rustix::termios::tcgetattr(&*writer) else {
        return;
    };
    let end = settings.special_codes[rustix::termios::SpecialCodeIndex::VEOF];
    if end != 0 {
        drop(writer.write_all(&[b'\n', end]));
    }
}

#[cfg(unix)]
pub(super) fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;

    status
        .code()
        .or_else(|| status.signal().map(|signal| 128_i32.saturating_add(signal)))
        .unwrap_or(1)
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

enum Guarded {
    #[cfg(unix)]
    Group(Pid),
    #[cfg(not(unix))]
    Group(u32),
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Running(Arc<dyn crate::embedded::Running>),
}

pub struct ProcessGroupGuard {
    guarded: Option<Guarded>,
}

impl ProcessGroupGuard {
    pub fn new(pid: u32) -> Self {
        #[cfg(unix)]
        let guarded = i32::try_from(pid)
            .ok()
            .map(|pid| Guarded::Group(Pid::from_raw(pid)));
        #[cfg(not(unix))]
        let guarded = Some(Guarded::Group(pid));
        Self { guarded }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn embedded(running: Arc<dyn crate::embedded::Running>) -> Self {
        Self {
            guarded: Some(Guarded::Running(running)),
        }
    }

    #[cfg(all(
        unix,
        not(any(
            target_os = "openbsd",
            target_os = "redox",
            target_os = "horizon",
            target_os = "cygwin"
        ))
    ))]
    fn prepare_to_reap(&mut self) -> io::Result<bool> {
        use rustix::process::{WaitId, WaitIdOptions, waitid};

        let Some(Guarded::Group(group)) = &self.guarded else {
            return Ok(true);
        };
        let pid = rustix::process::Pid::from_raw(group.as_raw())
            .ok_or_else(|| io::Error::other("invalid shell process identifier"))?;
        match waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(None) => Ok(false),
            Ok(Some(_)) => {
                let _ = self.terminate_and_disarm();
                Ok(true)
            }
            Err(error) => Err(error.into()),
        }
    }

    #[cfg(not(all(
        unix,
        not(any(
            target_os = "openbsd",
            target_os = "redox",
            target_os = "horizon",
            target_os = "cygwin"
        ))
    )))]
    fn prepare_to_reap(&mut self) -> io::Result<bool> {
        Ok(true)
    }

    #[cfg(unix)]
    pub(super) fn interrupt(&self) -> io::Result<()> {
        match &self.guarded {
            None => Ok(()),
            Some(Guarded::Group(process_group)) => match killpg(*process_group, Signal::SIGINT) {
                Ok(()) | Err(Errno::ESRCH) => Ok(()),
                Err(error) => Err(io::Error::from_raw_os_error(error as i32)),
            },
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Some(Guarded::Running(running)) => {
                running.interrupt();
                Ok(())
            }
        }
    }

    #[cfg(not(unix))]
    pub(super) fn interrupt(&self) -> io::Result<()> {
        self.terminate()
    }

    #[cfg(unix)]
    fn terminate(&self) -> io::Result<()> {
        let process_group = match &self.guarded {
            None => return Ok(()),
            Some(Guarded::Group(process_group)) => *process_group,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Some(Guarded::Running(running)) => {
                running.stop();
                return Ok(());
            }
        };
        for signal in [Signal::SIGSTOP, Signal::SIGKILL] {
            match killpg(process_group, signal) {
                Ok(()) => {}
                Err(Errno::ESRCH) => return Ok(()),
                Err(error) => return Err(io::Error::from_raw_os_error(error as i32)),
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn terminate(&self) -> io::Result<()> {
        let Some(Guarded::Group(process_group)) = &self.guarded else {
            return Ok(());
        };

        std::process::Command::new("taskkill.exe")
            .args(["/PID", &process_group.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|_| ())
    }

    #[cfg(not(any(unix, windows)))]
    fn terminate(&self) -> io::Result<()> {
        Ok(())
    }

    pub(super) fn disarm(&mut self) {
        self.guarded = None;
    }

    pub fn terminate_and_disarm(&mut self) -> io::Result<()> {
        let result = self.terminate();
        self.disarm();
        result
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

pub(super) fn sanitized_environment(
    overrides: &[(OsString, OsString)],
    secret_overrides: &[(OsString, OsString)],
) -> (Vec<(OsString, OsString)>, Redactor) {
    let mut environment = Vec::new();
    let mut redactor = Redactor::raw(MIN_REDACTED_SECRET_BYTES);
    for (name, value) in secret_overrides {
        if let Some(value) = value.to_str() {
            redactor.add(value, &format!("[secret {}]", name.to_string_lossy()));
        }
    }
    for (name, value) in env::vars_os() {
        if crate::SessionEnvironment::controls(&name) {
            continue;
        }
        if is_sensitive_name(&name) {
            if let Some(value) = value.to_str().filter(|value| value.len() >= 8) {
                redactor.add(value, REDACTED);
            }
        } else {
            environment.push((name, value));
        }
    }
    normalize_environment(&mut environment);
    for (name, value) in crate::command_environment().iter().chain(overrides) {
        environment.retain(|(candidate, _)| candidate != name);
        environment.push((name.clone(), value.clone()));
        if is_sensitive_name(name)
            && let Some(value) = value.to_str().filter(|value| value.len() >= 8)
        {
            redactor.add(value, REDACTED);
        }
    }
    for (name, value) in secret_overrides {
        environment.retain(|(candidate, _)| candidate != name);
        environment.push((name.clone(), value.clone()));
    }
    (environment, redactor)
}

fn normalize_environment(environment: &mut Vec<(OsString, OsString)>) {
    environment.retain(|(candidate, _)| {
        NORMALIZED_ENVIRONMENT
            .iter()
            .all(|(name, _)| candidate != name)
    });
    for (name, value) in NORMALIZED_ENVIRONMENT {
        environment.push((name.into(), value.into()));
    }
}

#[must_use]
pub fn ambient_sensitive_environment() -> Vec<(OsString, OsString)> {
    env::vars_os()
        .filter(|(name, _)| is_sensitive_name(name))
        .collect()
}

fn is_sensitive_name(name: &OsStr) -> bool {
    name.as_encoded_bytes()
        .split(|byte| *byte == b'_')
        .any(|part| {
            SENSITIVE_ENV_PARTS
                .iter()
                .any(|sensitive| part.eq_ignore_ascii_case(sensitive.as_bytes()))
        })
}
