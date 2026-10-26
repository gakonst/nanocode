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
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
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
    wait: Option<JoinHandle<io::Result<i32>>>,
    completed: Option<io::Result<i32>>,
    process_group: SharedProcessGroup,
    exited: Arc<AtomicBool>,
}

impl ProcessChild {
    fn new(
        mut try_wait: impl FnMut() -> io::Result<Option<i32>> + Send + 'static,
        pid: u32,
        io_driver_available: bool,
    ) -> Self {
        #[cfg(not(unix))]
        let _ = io_driver_available;
        let (commands, mut received) = mpsc::unbounded_channel();
        let process_group = SharedProcessGroup(commands);
        let mut guard = ProcessGroupGuard::new(pid);
        let exited = Arc::new(AtomicBool::new(false));
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
                        reaped.store(true, Ordering::SeqCst);
                        return Ok(status);
                    }

                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        guard.disarm();
                        reaped.store(true, Ordering::SeqCst);
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
            wait: Some(wait),
            completed: None,
            process_group,
            exited,
        }
    }

    pub(super) fn exited(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exited)
    }

    pub(super) async fn wait(&mut self) -> io::Result<i32> {
        if self.completed.is_none() {
            let result = self
                .wait
                .as_mut()
                .ok_or_else(|| io::Error::other("shell wait result is unavailable"))?
                .await
                .map_err(|error| io::Error::other(format!("shell wait task failed: {error}")))
                .and_then(|result| result);
            self.wait = None;
            self.completed = Some(result);
        }
        match self.completed.as_ref().expect("completed wait") {
            Ok(status) => Ok(*status),
            Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
        }
    }
}

impl Drop for ProcessChild {
    fn drop(&mut self) {
        self.process_group.terminate_and_disarm();
    }
}

const REAP_POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    fn pty(mut writer: Box<dyn Write + Send>) -> Self {
        let (sender, mut writes) = mpsc::unbounded_channel::<PtyWrite>();
        drop(tokio::task::spawn_blocking(move || {
            while let Some((bytes, reply)) = writes.blocking_recv() {
                drop(reply.send(writer.write_all(&bytes).and_then(|()| writer.flush())));
            }
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
        return spawn_pty(script, workspace, shell, login, environment);
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

fn spawn_pty(
    script: &str,
    workspace: &Path,
    shell: &Shell,
    login: bool,
    environment: &[(OsString, OsString)],
) -> io::Result<SpawnedProcess> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(pty_error)?;
    let mut command = CommandBuilder::new(shell.path());
    for argument in shell.args(script, login) {
        command.arg(argument);
    }
    command.cwd(workspace);
    command.env_clear();
    for (name, value) in environment {
        command.env(name, value);
    }

    let reader = pair.master.try_clone_reader().map_err(pty_error)?;
    let writer = pair.master.take_writer().map_err(pty_error)?;
    let mut child = pair.slave.spawn_command(command).map_err(pty_error)?;
    let pid = child
        .process_id()
        .ok_or_else(|| io::Error::other("spawned PTY command without a process identifier"))?;
    let child = ProcessChild::new(
        move || {
            child.try_wait().map(|status| {
                status.map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX))
            })
        },
        pid,
        false,
    );

    Ok(SpawnedProcess {
        process_group: child.process_group.clone(),
        child,
        stdin: Some(ProcessStdin::pty(writer)),
        output: ProcessOutput::Pty(reader),
    })
}

fn pty_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
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

pub struct ProcessGroupGuard {
    #[cfg(unix)]
    process_group: Option<Pid>,
    #[cfg(not(unix))]
    process_group: Option<u32>,
}

impl ProcessGroupGuard {
    pub fn new(pid: u32) -> Self {
        #[cfg(unix)]
        let process_group = i32::try_from(pid).ok().map(Pid::from_raw);
        #[cfg(not(unix))]
        let process_group = Some(pid);
        Self { process_group }
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

        let Some(group) = self.process_group else {
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
        let Some(process_group) = self.process_group else {
            return Ok(());
        };
        match killpg(process_group, Signal::SIGINT) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(io::Error::from_raw_os_error(error as i32)),
        }
    }

    #[cfg(not(unix))]
    pub(super) fn interrupt(&self) -> io::Result<()> {
        self.terminate()
    }

    #[cfg(unix)]
    fn terminate(&self) -> io::Result<()> {
        let Some(process_group) = self.process_group else {
            return Ok(());
        };
        for signal in [Signal::SIGSTOP, Signal::SIGKILL] {
            match killpg(process_group, signal) {
                Ok(()) | Err(Errno::ESRCH) => {}
                Err(error) => return Err(io::Error::from_raw_os_error(error as i32)),
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn terminate(&self) -> io::Result<()> {
        let Some(process_group) = self.process_group else {
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

    pub(super) const fn disarm(&mut self) {
        self.process_group = None;
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

/// The environment for a command and a redactor for its output: inherited or
/// overridden values of sensitive-looking variables show as `[REDACTED]`,
/// and each of `secret_overrides` as `[secret NAME]`.
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
    for (name, value) in overrides {
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

/// The script a login shell runs so that `secret_overrides` win over its
/// profile: a profile that sets a variable of the same name runs after the
/// environment is applied and would replace the host's value. For POSIX
/// shells each secret is also passed under a private name and exported again
/// from it before the script; other shells and non-login shells read no
/// profile this way and run the script unchanged. Names that are not shell
/// identifiers keep only the environment.
pub(super) fn export_secrets_after_profile<'a>(
    script: &'a str,
    shell: &Shell,
    login: bool,
    secret_overrides: &[(OsString, OsString)],
    environment: &mut Vec<(OsString, OsString)>,
) -> std::borrow::Cow<'a, str> {
    if !login || !matches!(shell.name(), "zsh" | "bash" | "sh") {
        return script.into();
    }
    let mut prefix = String::new();
    for (name, value) in secret_overrides {
        let Some(name) = name.to_str().filter(|name| is_shell_identifier(name)) else {
            continue;
        };
        let private = format!("{SECRET_COPY_PREFIX}{name}");
        environment.retain(|(candidate, _)| candidate != private.as_str());
        environment.push((private.clone().into(), value.clone()));
        // A read-only or special name (UID, PPID...) fails in a subshell
        // first, so it can't abort the command under zsh or sh.
        prefix.push_str(&format!(
            "( export {name}=\"${private}\" ) 2>/dev/null && export {name}=\"${private}\"; unset {private}; "
        ));
    }
    if prefix.is_empty() {
        return script.into();
    }
    // One line, so the script's line numbers are unchanged.
    prefix.push_str(script);
    prefix.into()
}

/// Prefix of the private copies `export_secrets_after_profile` passes.
const SECRET_COPY_PREFIX: &str = "__NANOCODE_SECRET_";

fn is_shell_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn normalize_environment(environment: &mut Vec<(OsString, OsString)>) {
    for (name, value) in NORMALIZED_ENVIRONMENT {
        environment.retain(|(candidate, _)| candidate != name);
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
    name.to_string_lossy()
        .to_ascii_uppercase()
        .split('_')
        .any(|part| SENSITIVE_ENV_PARTS.contains(&part))
}
