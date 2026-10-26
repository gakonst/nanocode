#![cfg(not(target_family = "wasm"))]

pub use nanocode_home::SessionEnvironment;

mod foreground;
pub use foreground::{ForegroundOutput, run_bash};
mod output;
mod process;
use nanocode_home::shell as selection;

pub use process::ProcessGroupGuard;

pub use process::ambient_sensitive_environment;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use nanocode_sync::{Actor, Atom};
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};

pub use nanocode_process_contracts::{
    DEFAULT_EXEC_YIELD_MS, DEFAULT_POLL_YIELD_MS, DEFAULT_WRITE_YIELD_MS, MAX_EXEC_YIELD_MS,
    MAX_POLL_YIELD_MS,
};
const DRAIN_GRACE: Duration = Duration::from_secs(2);
const MAX_SESSIONS: usize = 64;
const PROTECTED_SESSIONS: usize = 8;

pub use nanocode_process_contracts::{
    ExecCommand, ExecCommandResult, ForegroundCommand, ForegroundExecutor, LastActivity,
    ProcessExecutors, RunningProcess, ShellExecutor, WriteStdin,
};

#[derive(Clone, Default)]
pub struct NativeProcessExecutors {
    environment: Arc<Vec<(OsString, OsString)>>,
}

impl NativeProcessExecutors {
    #[must_use]
    pub fn with_environment(environment: Vec<(OsString, OsString)>) -> Self {
        Self {
            environment: Arc::new(environment),
        }
    }
}

impl ProcessExecutors for NativeProcessExecutors {
    fn shell(&self, environment: Vec<(OsString, OsString)>) -> Arc<dyn ShellExecutor> {
        let environment = self
            .environment
            .iter()
            .cloned()
            .chain(environment)
            .collect();
        Arc::new(ShellSessions::with_environment(Arc::new(environment)))
    }

    fn foreground(&self) -> Arc<dyn ForegroundExecutor> {
        match self.environment.is_empty() {
            true => Arc::new(NativeForeground),
            false => Arc::new(EnvironmentForeground(Arc::clone(&self.environment))),
        }
    }
}

struct EnvironmentForeground(Arc<Vec<(OsString, OsString)>>);

impl ForegroundExecutor for EnvironmentForeground {
    fn execute<'a>(
        &'a self,
        request: ForegroundCommand<'a>,
    ) -> nanocode_process_contracts::ProcessFuture<'a, Result<ForegroundOutput, String>> {
        foreground::run_bash_with_environment(
            request.workspace,
            request.command,
            request.timeout,
            request.max_stdout,
            request.max_stderr,
            request.session,
            &self.0,
            request.output,
        )
    }
}

type Sessions = Arc<Atom<HashMap<i64, Arc<Session>>>>;

pub type SecretEnvironment = Arc<dyn Fn() -> Vec<(OsString, OsString)> + Send + Sync>;

pub struct ShellSessions {
    terminations: AtomicU64,
    uses: AtomicU64,
    sessions: Sessions,
    next_session_id: AtomicI64,
    default_shell: selection::Shell,
    environment: Arc<Vec<(OsString, OsString)>>,
    secrets: Option<SecretEnvironment>,
    per_caller: bool,
}

impl ShellSessions {
    pub fn with_environment(environment: Arc<Vec<(OsString, OsString)>>) -> Self {
        Self {
            terminations: AtomicU64::new(0),
            uses: AtomicU64::new(0),
            sessions: Arc::default(),
            next_session_id: AtomicI64::new(1),
            default_shell: selection::default_user_shell(),
            environment,
            secrets: None,
            per_caller: false,
        }
    }

    #[must_use]
    pub const fn per_caller(mut self) -> Self {
        self.per_caller = true;
        self
    }

    pub fn with_secrets(
        environment: Arc<Vec<(OsString, OsString)>>,
        secrets: SecretEnvironment,
    ) -> Self {
        Self {
            secrets: Some(secrets),
            ..Self::with_environment(environment)
        }
    }

    pub const fn default_shell_name(&self) -> &'static str {
        self.default_shell.name()
    }

    pub fn execute<'call>(
        &'call self,
        command: ExecCommand,
        workspace: &'call Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ExecCommandResult> + Send + 'call>>
    {
        Box::pin(async move {
            let started_at = Instant::now();
            let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
            let workdir = resolve_workdir(workspace, command.workdir.as_deref());
            let shell = command.shell.as_deref().map_or_else(
                || self.default_shell.clone(),
                selection::get_shell_by_model_provided_path,
            );
            let host_secrets = self
                .secrets
                .as_ref()
                .map_or_else(Vec::new, |secrets| secrets());
            let (mut environment, secrets) =
                process::sanitized_environment(&self.environment, &host_secrets);
            let login = command.login.unwrap_or(true);
            let script = process::export_secrets_after_profile(
                &command.script,
                &shell,
                login,
                &host_secrets,
                &mut environment,
            );

            let terminations = self.terminations.load(Ordering::SeqCst);
            let spawned =
                match process::spawn(&script, &workdir, &shell, login, command.tty, &environment) {
                    Ok(spawned) => spawned,
                    Err(error) => {
                        return ExecCommandResult::failed(
                            started_at.elapsed(),
                            format!(
                                "exec_command failed for `{}`: CreateProcess {{ message: {:?} }}",
                                command.script,
                                error.to_string()
                            ),
                        );
                    }
                };
            let launch = Launch {
                command: command.script.clone(),
                workdir,
                started_at: SystemTime::now(),
                caller: command.caller,
            };
            let session = match Session::new(session_id, spawned, secrets, launch) {
                Ok(session) => session,
                Err(error) => {
                    return ExecCommandResult::failed(
                        started_at.elapsed(),
                        format!("exec_command failed for `{}`: {error}", command.script),
                    );
                }
            };
            let mut cancellation = CancelExecution {
                session: Some(Arc::clone(&session)),
                sessions: Arc::clone(&self.sessions),
            };
            let yield_time = duration_ms(
                command.yield_time_ms,
                DEFAULT_EXEC_YIELD_MS,
                250,
                MAX_EXEC_YIELD_MS,
            );
            let max_output_tokens = command.max_output_tokens;
            session
                .last_used
                .store(self.uses.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
            let busy = Busy::new(&session);
            let initial = session.owner.run(move |state| {
                Box::pin(state.wait_for_output(yield_time, max_output_tokens, started_at))
            });
            let pruned = self.sessions.update(|sessions| {
                let pruned = prune(sessions);
                sessions.insert(session_id, Arc::clone(&session));
                pruned
            });
            if let Some(pruned) = pruned {
                tokio::spawn(async move {
                    pruned.terminate().await;
                });
            }
            if self.terminations.load(Ordering::SeqCst) != terminations {
                self.forget(session_id);
                session.process_group.terminate_and_disarm();
            }

            let result = initial
                .await
                .unwrap_or_else(|_| session.stopped(started_at));
            drop(busy);
            cancellation.session = None;
            if result.exit_code.is_some() {
                self.forget(session_id);
            }
            result
        })
    }

    pub fn write_stdin<'call>(
        &'call self,
        request: WriteStdin,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ExecCommandResult> + Send + 'call>>
    {
        Box::pin(async move {
            let started_at = Instant::now();
            let session_id = i64::from(request.session_id);
            let session = self
                .sessions
                .read(|sessions| sessions.get(&session_id).cloned());
            let Some(session) = session else {
                return ExecCommandResult::failed(
                    started_at.elapsed(),
                    format!(
                        "write_stdin failed: Unknown process id {}",
                        request.session_id
                    ),
                );
            };
            if self.per_caller && session.launch.caller != request.caller {
                return ExecCommandResult::failed(
                    started_at.elapsed(),
                    format!(
                        "write_stdin failed: process id {} belongs to another session",
                        request.session_id
                    ),
                );
            }

            if !request.chars.is_empty() && !session.tty && request.chars != "\u{3}" {
                return ExecCommandResult::failed(
                    started_at.elapsed(),
                    "write_stdin failed: stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
                        .to_owned(),
                );
            }
            let (default, minimum, maximum) = if request.chars.is_empty() {
                (DEFAULT_POLL_YIELD_MS, 5_000, MAX_POLL_YIELD_MS)
            } else {
                (DEFAULT_WRITE_YIELD_MS, 250, MAX_EXEC_YIELD_MS)
            };
            let yield_time = duration_ms(request.yield_time_ms, default, minimum, maximum);
            let max_output_tokens = request.max_output_tokens;
            let chars = request.chars;
            if !chars.is_empty() {
                session.activity.touch();
            }
            session
                .last_used
                .store(self.uses.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
            let busy = Busy::new(&session);
            let result = session
                .owner
                .run(move |state| {
                    Box::pin(async move {
                        if !chars.is_empty() {
                            let written = if state.tty {
                                state.write(&chars).await
                            } else {
                                state.process_group.interrupt().await
                            };
                            if let Err(error) = written {
                                return ExecCommandResult::failed(
                                    started_at.elapsed(),
                                    format!("write_stdin failed: {error}"),
                                );
                            }
                        }
                        state
                            .wait_for_output(yield_time, max_output_tokens, started_at)
                            .await
                    })
                })
                .await
                .unwrap_or_else(|_| session.stopped(started_at));
            drop(busy);
            if result.exit_code.is_some() {
                self.forget(session_id);
            }
            result
        })
    }

    pub fn running(&self) -> Vec<RunningProcess> {
        let mut running = self.sessions.read(|sessions| {
            sessions
                .values()
                .filter(|session| !session.exited.load(Ordering::SeqCst))
                .map(|session| RunningProcess {
                    session_id: session.id,
                    caller: session.launch.caller.clone(),
                    command: session.launch.command.clone(),
                    workdir: session.launch.workdir.clone(),
                    started_at: session.launch.started_at,
                    in_call: session.busy.load(Ordering::SeqCst) > 0,
                    silent_for: session.activity.silent_for(),
                })
                .collect::<Vec<_>>()
        });
        running.sort_unstable_by_key(|process| process.session_id);
        running
    }

    pub fn terminate_all<'call>(
        &'call self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'call>> {
        Box::pin(async move {
            self.terminations.fetch_add(1, Ordering::SeqCst);
            let sessions = self.sessions.update(std::mem::take);
            for session in sessions.into_values() {
                session.terminate().await;
            }
        })
    }

    fn forget(&self, session_id: i64) {
        self.sessions.update(|sessions| {
            sessions.remove(&session_id);
        });
    }
}

struct Launch {
    command: String,
    workdir: PathBuf,
    started_at: SystemTime,
    caller: Option<String>,
}

struct Session {
    id: i64,
    tty: bool,
    process_group: process::SharedProcessGroup,
    owner: Actor<SessionState>,
    launch: Launch,
    last_used: AtomicU64,
    busy: AtomicUsize,
    exited: Arc<AtomicBool>,
    activity: Arc<LastActivity>,
}

struct Busy<'a>(&'a Session);

impl<'a> Busy<'a> {
    fn new(session: &'a Session) -> Self {
        session.busy.fetch_add(1, Ordering::SeqCst);
        Self(session)
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::SeqCst);
    }
}

fn prune(sessions: &mut HashMap<i64, Arc<Session>>) -> Option<Arc<Session>> {
    if sessions.len() < MAX_SESSIONS {
        return None;
    }
    let mut recent = sessions
        .values()
        .map(|session| (session.last_used.load(Ordering::Relaxed), session.id))
        .collect::<Vec<_>>();
    recent.sort_unstable_by(|left, right| right.cmp(left));
    let protected = recent
        .iter()
        .take(PROTECTED_SESSIONS)
        .map(|(_, id)| *id)
        .collect::<HashSet<_>>();
    let candidates = || {
        recent.iter().rev().filter(|(_, id)| {
            !protected.contains(id) && sessions[id].busy.load(Ordering::SeqCst) == 0
        })
    };
    let victim = candidates()
        .find(|(_, id)| sessions[id].exited.load(Ordering::SeqCst))
        .or_else(|| candidates().next())
        .map(|(_, id)| *id)?;
    sessions.remove(&victim)
}

struct SessionState {
    id: i64,
    tty: bool,
    child: process::ProcessChild,
    stdin: Option<process::ProcessStdin>,
    process_group: process::SharedProcessGroup,
    drains: Option<Vec<JoinHandle<()>>>,
    output: mpsc::UnboundedReceiver<Vec<u8>>,
    captured: CapturedOutput,
    secrets: nanocode_process_contracts::Redactor,
}

impl Session {
    fn new(
        id: i64,
        spawned: process::SpawnedProcess,
        secrets: nanocode_process_contracts::Redactor,
        launch: Launch,
    ) -> std::io::Result<Arc<Self>> {
        let tty = matches!(spawned.stdin.as_ref(), Some(process::ProcessStdin::Pty(_)));
        let (chunks, output) = mpsc::unbounded_channel();
        let activity = Arc::new(LastActivity::new());
        let drains = match spawned.output {
            process::ProcessOutput::Pipes { stdout, stderr } => vec![
                tokio::spawn(output::drain(stdout, chunks.clone(), Arc::clone(&activity))),
                tokio::spawn(output::drain(stderr, chunks, Arc::clone(&activity))),
            ],
            process::ProcessOutput::Pty(reader) => vec![output::drain_blocking(
                reader,
                chunks,
                Arc::clone(&activity),
            )],
        };
        let process_group = spawned.process_group;
        let exited = spawned.child.exited();
        let owner = Actor::spawn(SessionState {
            id,
            tty,
            child: spawned.child,
            stdin: spawned.stdin,
            process_group: process_group.clone(),
            drains: Some(drains),
            output,
            captured: CapturedOutput::default(),
            secrets,
        })?;
        Ok(Arc::new(Self {
            id,
            tty,
            process_group,
            owner,
            launch,
            last_used: AtomicU64::new(0),
            busy: AtomicUsize::new(0),
            exited,
            activity,
        }))
    }

    fn stopped(&self, started_at: Instant) -> ExecCommandResult {
        ExecCommandResult::failed(
            started_at.elapsed(),
            format!("shell session {} stopped", self.id),
        )
    }

    async fn terminate(&self) {
        if let Err(error) = self.process_group.terminate().await {
            tracing::warn!(
                shell.session.id = self.id,
                %error,
                "failed to terminate shell process group"
            );
        }
        let _ = self.owner.run(|state| Box::pin(state.close())).await;
    }
}

impl SessionState {
    async fn close(&mut self) {
        self.stdin.take();
        if let Err(error) = self.child.wait().await {
            tracing::warn!(
                shell.session.id = self.id,
                %error,
                "failed to reap terminated shell process"
            );
        }
        self.finish_drains().await;
    }

    async fn write(&mut self, chars: &str) -> std::io::Result<()> {
        let stdin = self.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin is closed")
        })?;
        stdin.write(chars.as_bytes()).await
    }

    async fn wait_for_output(
        &mut self,
        yield_time: Duration,
        max_output_tokens: Option<usize>,
        started_at: Instant,
    ) -> ExecCommandResult {
        let status = timeout(yield_time, self.child.wait()).await;
        let exit_code = match status {
            Ok(Ok(exit_code)) => {
                self.process_group.terminate_and_disarm();
                self.finish_drains().await;
                Some(exit_code)
            }
            Ok(Err(error)) => {
                self.process_group.terminate_and_disarm();
                let message = format!("failed to wait for shell command: {error}");
                self.collect();
                self.captured.push(message.as_bytes(), usize::MAX);
                Some(1)
            }
            Err(_) => None,
        };
        let (output, original_token_count) = self.take_output(max_output_tokens);
        let chunk_id = match generate_chunk_id() {
            Ok(chunk_id) => Some(chunk_id),
            Err(error) => {
                tracing::warn!(%error, "failed to generate shell output chunk ID");
                None
            }
        };
        ExecCommandResult {
            error: None,
            chunk_id,
            wall_time_seconds: started_at.elapsed().as_secs_f64(),
            exit_code,
            session_id: exit_code.is_none().then_some(self.id),
            original_token_count,
            output,
        }
    }

    async fn finish_drains(&mut self) {
        let Some(handles) = self.drains.take() else {
            return;
        };
        let deadline = tokio::time::Instant::now() + DRAIN_GRACE;
        for mut handle in handles {
            if tokio::time::timeout_at(deadline, &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    fn collect(&mut self) {
        while let Ok(chunk) = self.output.try_recv() {
            self.captured.push(&chunk, usize::MAX);
        }
    }

    fn take_output(&mut self, max_output_tokens: Option<usize>) -> (String, Option<usize>) {
        self.collect();
        let captured = self.captured.take();
        let mut raw = String::from_utf8_lossy(&captured.with_omission_marker()).into_owned();
        self.secrets.text(&mut raw);
        let limit = output::effective_token_limit(max_output_tokens);
        let (output, _) = output::redact_and_limit(raw, &[], limit);
        (output, Some(captured.total_bytes.saturating_add(3) / 4))
    }
}

#[derive(Default)]
pub(crate) struct CapturedOutput {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    omitted_bytes: usize,
}

impl CapturedOutput {
    pub(crate) fn push(&mut self, bytes: &[u8], limit: usize) {
        if bytes.is_empty() {
            return;
        }
        if limit == 0 {
            self.omitted_bytes = self.omitted_bytes.saturating_add(bytes.len());
            return;
        }

        let head_budget = limit / 2;
        let tail_budget = limit.saturating_sub(head_budget);
        let head_len = head_budget.saturating_sub(self.head.len()).min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_len]);
        let tail_bytes = &bytes[head_len..];
        if tail_bytes.len() >= tail_budget {
            let kept_from = tail_bytes.len().saturating_sub(tail_budget);
            self.omitted_bytes = self
                .omitted_bytes
                .saturating_add(self.tail.len())
                .saturating_add(kept_from);
            self.tail.clear();
            self.tail.extend(&tail_bytes[kept_from..]);
        } else {
            self.tail.extend(tail_bytes);
            let excess = self.tail.len().saturating_sub(tail_budget);
            if excess > 0 {
                drop(self.tail.drain(..excess));
                self.omitted_bytes = self.omitted_bytes.saturating_add(excess);
            }
        }
    }

    fn take(&mut self) -> CapturedChunk {
        let head = std::mem::take(&mut self.head);
        let tail = std::mem::take(&mut self.tail);
        let omitted_bytes = std::mem::take(&mut self.omitted_bytes);
        CapturedChunk {
            total_bytes: head
                .len()
                .saturating_add(tail.len())
                .saturating_add(omitted_bytes),
            head,
            tail,
            omitted_bytes,
        }
    }
}

struct CapturedChunk {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    omitted_bytes: usize,
    total_bytes: usize,
}

impl CapturedChunk {
    fn with_omission_marker(&self) -> Vec<u8> {
        let marker = (self.omitted_bytes > 0)
            .then(|| format!("... {} bytes omitted ...", self.omitted_bytes));
        let marker_length = marker.as_ref().map_or(0, |marker| marker.len() + 2);
        let mut output = Vec::with_capacity(
            self.head
                .len()
                .saturating_add(self.tail.len())
                .saturating_add(marker_length),
        );
        output.extend_from_slice(&self.head);
        if let Some(marker) = marker {
            output.push(b'\n');
            output.extend_from_slice(marker.as_bytes());
            output.push(b'\n');
        }
        output.extend(self.tail.iter().copied());
        output
    }
}

fn resolve_workdir(workspace: &Path, requested: Option<&str>) -> PathBuf {
    let requested = requested.filter(|workdir| !workdir.is_empty());
    match requested.map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => workspace.join(path),
        None => workspace.to_owned(),
    }
}

fn duration_ms(requested: Option<u64>, default: u64, minimum: u64, maximum: u64) -> Duration {
    let requested = requested.unwrap_or(default);
    Duration::from_millis(requested.clamp(minimum, maximum))
}

fn generate_chunk_id() -> Result<String, getrandom::Error> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 3];
    getrandom::fill(&mut bytes)?;
    Ok(bytes
        .into_iter()
        .flat_map(|byte| {
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 0x0f)]),
            ]
        })
        .collect())
}

struct CancelExecution {
    session: Option<Arc<Session>>,
    sessions: Sessions,
}
impl Drop for CancelExecution {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            self.sessions.update(|sessions| {
                sessions.remove(&session.id);
            });
            tokio::spawn(async move {
                session.terminate().await;
            });
        }
    }
}

pub use nanocode_home::shell::default_shell_name;

impl ShellExecutor for ShellSessions {
    fn execute<'a>(
        &'a self,
        command: ExecCommand,
        workspace: &'a Path,
    ) -> nanocode_process_contracts::ProcessFuture<'a, ExecCommandResult> {
        Self::execute(self, command, workspace)
    }
    fn write_stdin(
        &self,
        request: WriteStdin,
    ) -> nanocode_process_contracts::ProcessFuture<'_, ExecCommandResult> {
        Self::write_stdin(self, request)
    }
    fn terminate_all(&self) -> nanocode_process_contracts::ProcessFuture<'_, ()> {
        Self::terminate_all(self)
    }
    fn running(&self) -> Vec<RunningProcess> {
        Self::running(self)
    }
}

pub struct NativeForeground;
impl ForegroundExecutor for NativeForeground {
    fn execute<'a>(
        &'a self,
        request: ForegroundCommand<'a>,
    ) -> nanocode_process_contracts::ProcessFuture<'a, Result<ForegroundOutput, String>> {
        run_bash(
            request.workspace,
            request.command,
            request.timeout,
            request.max_stdout,
            request.max_stderr,
            request.session,
            request.output,
        )
    }
}
