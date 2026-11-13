use crate::{
    agent::Steer,
    tools::{
        BashExecution, BashExecutor, BashRequest, BashResult, ClaudeBash,
        bash::DEFAULT_BACKGROUND_TIMEOUT_MS,
    },
};
use nanocode_agent_core::{Agent, PromptRequest, WeakAgent};
use nanocode_process_contracts::{ForegroundExecutor, LastActivity, OutputSink};
use nanocode_sync::{Atom, Signal, Slot};
use nanocode_tool_host::{
    SessionEnvironment, Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult,
    ToolsBuilder, contract::RunningTask,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
};

const STALL_POLL: Duration = Duration::from_secs(5);
const STALL_SILENCE: Duration = Duration::from_secs(45);
const STALL_TAIL_BYTES: u64 = 1024;
const DEADLINE_NOTE: &str = "If the work in progress still needs it, start it again with `run_in_background` and a longer `timeout`. If it already had the longest `timeout` allowed, do not restart it. Either way, report that it was stopped.";
const OUTPUT_CAP_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const OUTPUT_CAP_MARKER: &str = " [output truncated: exceeded 5GB disk cap] ";
const ENDS_WITH_TURN: &str = "If it exits while you are still working you will be notified, but it is terminated when you give your final response and no notification can follow that \u{2014} so do not end your turn to wait for it; if you need its result, wait for it before giving your final response.";
const REPORTED: &str = "Its end is reported with its exit status and last output: an exec or wait call that is waiting when it finishes returns then, and otherwise the next one reports it.";
const SHELL_SIGNAL_STATUS: i32 = 128;

type Outcome = Result<BashResult, String>;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
enum Phase {
    Foreground,
    Background,
    Delivered,
}

struct AtomicPhase(AtomicU8);

impl AtomicPhase {
    const fn new() -> Self {
        Self(AtomicU8::new(Phase::Foreground as u8))
    }

    const fn decode(phase: u8) -> Phase {
        match phase {
            0 => Phase::Foreground,
            1 => Phase::Background,
            _ => Phase::Delivered,
        }
    }

    fn get(&self) -> Phase {
        Self::decode(self.0.load(Ordering::SeqCst))
    }

    fn advance(&self, phase: Phase) -> Phase {
        Self::decode(self.0.fetch_max(phase as u8, Ordering::SeqCst))
    }
}
struct Job {
    result: watch::Receiver<Option<Outcome>>,
    finished: Arc<Signal>,
    abort: tokio::task::AbortHandle,
    phase: AtomicPhase,
    expired: Arc<AtomicBool>,
    capped: Arc<Signal>,
    written: Arc<Signal>,
    activity: Arc<LastActivity>,
    id: String,
    command: String,
    label: String,
    call_id: Option<String>,
    owner: Option<String>,
    output: Option<PathBuf>,
    oom_kills: Option<u64>,
    notice: Slot<String>,
}
impl Job {
    fn status(&self) -> &'static str {
        match &*self.result.borrow() {
            _ if self.expired.load(Ordering::SeqCst) || self.capped.is_fired() => "killed",
            Some(Ok(result)) if result.exit_code == 0 => "completed",
            Some(_) => "failed",
            None if self.abort.is_finished() => "killed",
            None => "running",
        }
    }
    fn notification(
        &self,
        status: Option<&str>,
        summary: &str,
        body: &str,
        trailing: &str,
    ) -> String {
        let label = self
            .label
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let summary = format!("Background command \"{label}\" {summary}");
        let output = self.output.as_ref().map(|path| path.display().to_string());
        let mut text = "<task-notification>".to_owned();
        for (tag, value) in [
            ("task-id", Some(self.id.as_str())),
            ("tool-use-id", self.call_id.as_deref()),
            ("output-file", output.as_deref()),
            ("status", status),
            ("summary", Some(summary.as_str())),
        ] {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                text.push_str(&format!("\n<{tag}>{value}</{tag}>"));
            }
        }
        text.push_str(body);
        text.push_str("\n</task-notification>");
        text.push_str(trailing);
        text
    }
    async fn completion_notification(&self, outcome: &Outcome) -> String {
        let ending = self.ending(outcome).await;
        if self.expired.load(Ordering::SeqCst) {
            return self.notification(
                Some("killed"),
                "was stopped after reaching its background time limit",
                &format!("\n<note>{DEADLINE_NOTE}</note>"),
                &ending,
            );
        }
        if self.capped.is_fired() {
            return self.notification(Some("killed"), "was stopped", "", &ending);
        }
        let (status, summary) = match outcome {
            Ok(result) if result.exit_code == 0 => {
                ("completed", "completed (exit code 0)".to_owned())
            }
            Ok(result) => (
                "failed",
                match signal_name(result.exit_code) {
                    Some(signal) => format!(
                        "failed with exit code {}, a shell's status for a command that {signal} ended",
                        result.exit_code
                    ),
                    None => format!("failed with exit code {}", result.exit_code),
                },
            ),
            Err(_) => ("failed", "failed".to_owned()),
        };
        self.notification(Some(status), &summary, "", &ending)
    }
    async fn memory_kills(&self) -> Option<String> {
        let (before, after) = (self.oom_kills?, oom_kills().await?);
        (after > before).then(|| {
            format!(
                "The kernel killed {} process(es) in this machine's memory group for lack of memory while the command ran: oom_kill in memory.events went from {before} to {after}.",
                after - before
            )
        })
    }
    async fn ending(&self, outcome: &Outcome) -> String {
        let mut text = String::new();
        let killed =
            matches!(outcome, Ok(result) if signal_name(result.exit_code) == Some("SIGKILL"));
        match self.memory_kills().await {
            Some(kills) => text.push_str(&format!("\n{kills}")),
            None if killed && self.oom_kills.is_none() => text.push_str(
                "\nThe kernel also sends SIGKILL to a process when memory runs out; this machine's memory events cannot be read, so whether that happened here is a guess.",
            ),
            None => {}
        }
        let Some(path) = &self.output else {
            return text;
        };
        let Ok(metadata) = tokio::fs::metadata(path).await else {
            return text;
        };
        let Some(tail) = tail(path, metadata.len()).await else {
            return text;
        };
        let (lines, cut) = match tail.split_once('\n') {
            Some((_, lines)) if metadata.len() > STALL_TAIL_BYTES => (lines, true),
            _ => (tail.as_str(), metadata.len() > STALL_TAIL_BYTES),
        };
        if lines.trim().is_empty() {
            text.push_str("\nThe command printed nothing.");
        } else if cut {
            text.push_str(&format!(
                "\nLast output (the end of {} bytes, all in the output file):\n{}",
                metadata.len(),
                lines.trim_end()
            ));
        } else {
            text.push_str(&format!("\nOutput:\n{}", lines.trim_end()));
        }
        text
    }
}
fn signal_name(exit_code: i32) -> Option<&'static str> {
    Some(match exit_code.checked_sub(SHELL_SIGNAL_STATUS)? {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return None,
    })
}
async fn oom_kills() -> Option<u64> {
    nanocode_platform::spawn_blocking(|| {
        let groups = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let group = groups.lines().find_map(|line| line.strip_prefix("0::"))?;
        let events =
            std::fs::read_to_string(format!("/sys/fs/cgroup{group}/memory.events")).ok()?;
        events
            .lines()
            .find_map(|line| line.strip_prefix("oom_kill "))?
            .trim()
            .parse()
            .ok()
    })
    .await
    .ok()?
}
struct Finished(Arc<Signal>);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.fire();
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.abort.abort();
    }
}
struct TaskSink {
    file: Option<mpsc::UnboundedSender<Vec<u8>>>,
    activity: Arc<LastActivity>,
}
impl OutputSink for TaskSink {
    fn write(&self, bytes: &[u8]) {
        self.activity.touch();
        if let Some(file) = &self.file {
            let _ = file.send(bytes.to_vec());
        }
    }
}
async fn output_file(
    session: &str,
    id: &str,
    capped: Arc<Signal>,
) -> Option<(PathBuf, mpsc::UnboundedSender<Vec<u8>>, Arc<Signal>)> {
    let directory = std::env::temp_dir()
        .join("nanocode")
        .join(session)
        .join("tasks");
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(&directory).await.ok()?;
    let path = directory.join(format!("{id}.output"));
    let mut file = tokio::fs::File::create(&path).await.ok()?;
    let (sender, mut chunks) = mpsc::unbounded_channel::<Vec<u8>>();
    let written = Arc::new(Signal::new());
    let done = Arc::clone(&written);
    drop(nanocode_platform::spawn_task(async move {
        let mut total = 0_u64;
        while let Some(chunk) = chunks.recv().await {
            total = total.saturating_add(chunk.len() as u64);
            let over = total > OUTPUT_CAP_BYTES;
            let bytes = if over {
                OUTPUT_CAP_MARKER.as_bytes()
            } else {
                &chunk
            };
            if file.write_all(bytes).await.is_err() || over {
                if over {
                    capped.fire();
                }
                break;
            }
        }
        let _ = file.flush().await;
        done.fire();
    }));
    Some((path, sender, written))
}
async fn stall(job: &Job, binding: &Binding) {
    let Some(path) = &job.output else {
        return;
    };
    let mut size = 0;
    let mut quiet = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(STALL_POLL).await;
        let Ok(metadata) = tokio::fs::metadata(path).await else {
            continue;
        };
        if metadata.len() > size {
            size = metadata.len();
            quiet = tokio::time::Instant::now();
            continue;
        }
        if quiet.elapsed() < STALL_SILENCE {
            continue;
        }
        let Some(tail) = tail(path, metadata.len()).await else {
            continue;
        };
        if !prompting(&tail) {
            quiet = tokio::time::Instant::now();
            continue;
        }
        let text = job.notification(
            None,
            "appears to be waiting for interactive input",
            "",
            &format!(
                "\nLast output:\n{}\n\nThe command is likely blocked on an interactive prompt. Stop this task and re-run with piped input (e.g., `echo y | command`) or a non-interactive flag if one exists.",
                tail.trim_end()
            ),
        );
        route(binding.clone(), job.id.clone(), text);
        return;
    }
}
async fn tail(path: &Path, size: u64) -> Option<String> {
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let start = size.saturating_sub(STALL_TAIL_BYTES);
    file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
    let mut bytes = Vec::new();
    file.take(STALL_TAIL_BYTES)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    Some(without_escapes(&String::from_utf8_lossy(&bytes)))
}
fn without_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.next_if_eq(&'[').is_some() {
                while chars.next().is_some_and(|ch| !('@'..='~').contains(&ch)) {}
            }
            continue;
        }
        out.push(ch);
    }
    out
}
fn prompting(tail: &str) -> bool {
    let line = tail
        .trim_end()
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let question = line.trim_end_matches(' ').ends_with('?')
        && ["do you", "would you", "shall i", "are you sure", "ready to"]
            .iter()
            .any(|phrase| {
                line.match_indices(phrase).any(|(at, _)| {
                    let before = line[..at].chars().next_back();
                    let after = line[at + phrase.len()..].chars().next();
                    !before.is_some_and(char::is_alphanumeric)
                        && !after.is_some_and(char::is_alphanumeric)
                })
            });
    question
        || [
            "(y/n)",
            "[y/n]",
            "(yes/no)",
            "press any key",
            "press enter",
            "continue?",
            "overwrite?",
        ]
        .iter()
        .any(|pattern| line.contains(pattern))
}
fn route(binding: Binding, id: String, text: String) {
    drop(nanocode_platform::spawn_task(async move {
        if binding.ends_with_turn {
            if !(binding.steer)(text).await {
                tracing::debug!(task_id = %id, "background Bash notification found no running turn");
            }
            return;
        }
        let Some(agent) = binding.agent.upgrade() else {
            return;
        };
        if let Err(error) = agent.route_prompt_request(PromptRequest::new(text)).await {
            tracing::warn!(%error, task_id = %id, "background Bash notification did not reach the agent");
        }
    }));
}
fn whole_duration(milliseconds: u64) -> String {
    let minutes = milliseconds / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}
fn sleeps(command: &str) -> bool {
    command
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || ";&|()".contains(ch))
        .next()
        == Some("sleep")
}
#[derive(Clone, Copy)]
enum Watch {
    Notified,
    EndsWithTurn,
    Reported,
}
fn started(id: &str, output: Option<&Path>, timed_out: Option<Duration>, watch: Watch) -> String {
    let written = output.map_or_else(String::new, |path| {
        format!(" Output is being written to: {}.", path.display())
    });
    let mut parts = vec![match timed_out {
        Some(timeout) => format!(
            "Command did not complete within its {}s timeout and was moved to the background (ID: {id}).{written}",
            timeout.as_secs_f64().round().max(1.0)
        ),
        None => format!("Command running in background with ID: {id}.{written}"),
    }];
    parts.push(
        match watch {
            Watch::Notified => "You will be notified when it completes.",
            Watch::EndsWithTurn => ENDS_WITH_TURN,
            Watch::Reported => REPORTED,
        }
        .to_owned(),
    );
    if timed_out.is_some() {
        match watch {
            Watch::Notified => parts.push(format!(
                "If it is still running after {} in the background, it will be stopped and you will be notified.",
                whole_duration(DEFAULT_BACKGROUND_TIMEOUT_MS)
            )),
            Watch::Reported => parts.push(format!(
                "If it is still running after {} in the background, it will be stopped, which is reported the same way.",
                whole_duration(DEFAULT_BACKGROUND_TIMEOUT_MS)
            )),
            Watch::EndsWithTurn => {}
        }
    }
    if output.is_some() {
        parts.push("To check interim output, use Read on that file path.".to_owned());
    }
    parts.join(" ")
}
#[derive(Clone, Default)]
struct Entries {
    closed: bool,
    jobs: BTreeMap<String, Arc<Job>>,
}
#[derive(Clone)]
struct Binding {
    agent: WeakAgent,
    steer: Steer,
    ends_with_turn: bool,
}
#[derive(Default)]
pub(super) struct Jobs {
    next: AtomicU64,
    entries: Atom<Entries>,
    agent: Atom<Option<Binding>>,
    owners: Atom<BTreeMap<String, String>>,
    subagents: Option<Subagents>,
    ended: watch::Sender<u64>,
}
struct Subagents {
    list: Arc<dyn Tool>,
    close: Arc<dyn Tool>,
}
impl Jobs {
    pub(super) fn with_subagents(tools: &[Arc<dyn Tool>]) -> Self {
        let named = |name: &str| {
            tools
                .iter()
                .find(|tool| tool.definition().name() == name)
                .cloned()
        };
        Self {
            subagents: named("list_agents")
                .zip(named("close_agent"))
                .map(|(list, close)| Subagents { list, close }),
            ..Self::default()
        }
    }
    pub(super) fn bind(&self, agent: &Agent, steer: &Steer, ends_with_turn: bool) {
        let binding = Binding {
            agent: agent.downgrade(),
            steer: Arc::clone(steer),
            ends_with_turn,
        };
        self.agent.update(|bound| *bound = Some(binding.clone()));
    }
    pub(super) fn reap(&self) {
        let jobs = self
            .entries
            .read(|entries| entries.jobs.values().cloned().collect::<Vec<_>>());
        for job in jobs {
            if job.phase.get() >= Phase::Background && job.status() == "running" {
                job.abort.abort();
            }
        }
    }
    fn watch(self: &Arc<Self>, job: Arc<Job>) -> Watch {
        let Some(binding) = self.agent.read(Clone::clone) else {
            let jobs = Arc::downgrade(self);
            let (result, written) = (job.result.clone(), Arc::clone(&job.written));
            let job = Arc::downgrade(&job);
            drop(nanocode_platform::spawn_task(async move {
                let outcome = settled(result).await;
                written.wait().await;
                let Some(job) = job.upgrade() else {
                    return;
                };
                if job.result.borrow().is_none() {
                    return;
                }
                job.notice.put(job.completion_notification(&outcome).await);
                if let Some(jobs) = jobs.upgrade() {
                    jobs.ended.send_modify(|ended| *ended += 1);
                }
            }));
            return Watch::Reported;
        };
        let watch = if binding.ends_with_turn {
            Watch::EndsWithTurn
        } else {
            Watch::Notified
        };
        drop(nanocode_platform::spawn_task(async move {
            let finished = async {
                let outcome = completion(&job).await;
                job.written.wait().await;
                outcome
            };
            tokio::pin!(finished);
            let outcome = tokio::select! {
                biased;
                outcome = &mut finished => outcome,
                () = stall(&job, &binding) => finished.await,
            };
            if job.result.borrow().is_none()
                || job.phase.advance(Phase::Delivered) == Phase::Delivered
            {
                return;
            }
            let text = job.completion_notification(&outcome).await;
            route(binding, job.id.clone(), text);
        }));
        watch
    }
    fn reportable(&self, session_id: &str) -> Vec<Arc<Job>> {
        if self.agent.read(Option::is_some) {
            return Vec::new();
        }
        self.entries.read(|entries| {
            entries
                .jobs
                .values()
                .filter(|job| {
                    job.owner.as_deref() == Some(session_id)
                        && job.phase.get() == Phase::Background
                        && !job.notice.is_empty()
                })
                .cloned()
                .collect()
        })
    }
    async fn noticed(&self, session_id: &str) {
        let mut ended = self.ended.subscribe();
        while self.reportable(session_id).is_empty() {
            if ended.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
    fn notices(&self, session_id: &str) -> Vec<String> {
        self.reportable(session_id)
            .into_iter()
            .filter(|job| job.phase.advance(Phase::Delivered) != Phase::Delivered)
            .filter_map(|job| job.notice.take())
            .collect()
    }
    fn forget(&self, id: &str) {
        self.entries.update(|entries| {
            entries.jobs.remove(id);
        });
    }
    pub(super) async fn shutdown(&self) {
        let jobs = self.entries.update(|entries| {
            entries.closed = true;
            entries.jobs.values().cloned().collect::<Vec<_>>()
        });
        for job in &jobs {
            job.abort.abort();
        }
        for job in jobs {
            job.finished.wait().await;
        }
    }
}
fn discard(job: &Job) {
    let (Some(path), written) = (job.output.clone(), Arc::clone(&job.written)) else {
        return;
    };
    drop(nanocode_platform::spawn_task(async move {
        written.wait().await;
        let _ = tokio::fs::remove_file(path).await;
    }));
}
#[derive(Clone)]
struct LocalBash {
    workspace: PathBuf,
    jobs: Arc<Jobs>,
    executor: Arc<dyn ForegroundExecutor>,
}

async fn run(
    executor: &dyn ForegroundExecutor,
    workspace: &Path,
    request: &BashRequest,
    duration: Duration,
    output: Option<&dyn OutputSink>,
) -> Outcome {
    let output = executor
        .execute(nanocode_process_contracts::ForegroundCommand {
            workspace,
            command: &request.command,
            timeout: duration,
            max_stdout: request.max_stdout_bytes,
            max_stderr: request.max_stderr_bytes,
            session: request.session.as_ref(),
            output,
        })
        .await?;
    Ok(BashResult {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: output.exit_code,
        truncated: output.truncated,
    })
}
impl BashExecutor for LocalBash {
    async fn execute(&self, request: BashRequest) -> Outcome {
        let timeout = Duration::from_millis(request.timeout_ms);
        run(
            self.executor.as_ref(),
            &self.workspace,
            &request,
            timeout,
            None,
        )
        .await
    }
    async fn execute_with_background(
        &self,
        request: BashRequest,
        background: bool,
    ) -> Result<BashExecution, String> {
        let timeout = Duration::from_millis(request.timeout_ms);
        let max_stderr = request.max_stderr_bytes;
        let session = request
            .session
            .as_ref()
            .map_or("local", SessionEnvironment::session_id)
            .to_owned();
        let id = format!(
            "{session}-bash-{}",
            self.jobs.next.fetch_add(1, Ordering::Relaxed)
        );
        let capped = Arc::new(Signal::new());
        let activity = Arc::new(LastActivity::new());
        let (output, file, written) = match output_file(&session, &id, Arc::clone(&capped)).await {
            Some((path, file, written)) => (Some(path), Some(file), written),
            None => {
                let written = Arc::new(Signal::new());
                written.fire();
                (None, None, written)
            }
        };
        let sink = TaskSink {
            file,
            activity: Arc::clone(&activity),
        };
        let oom_kills = oom_kills().await;
        let (sender, receiver) = watch::channel(None);
        let (arm, armed) = oneshot::channel::<Duration>();
        let expired = Arc::new(AtomicBool::new(false));
        let job = {
            let workspace = self.workspace.clone();
            let executor = self.executor.clone();
            let finished = Arc::new(Signal::new());
            let admitted = Arc::new(Signal::new());
            let guard = Finished(Arc::clone(&finished));
            let start = Arc::clone(&admitted);
            let stopped = Arc::clone(&expired);
            let over = Arc::clone(&capped);
            let command = request.command.clone();
            let label = request
                .description
                .clone()
                .unwrap_or_else(|| command.clone());
            let call_id = request.call_id.clone();
            let owner = call_id
                .as_ref()
                .and_then(|id| self.jobs.owners.read(|owners| owners.get(id).cloned()));
            let task = nanocode_platform::spawn_task(async move {
                let _finished = guard;
                start.wait().await;
                let deadline = async move {
                    match armed.await {
                        Ok(after) => tokio::time::sleep(after).await,
                        Err(_) => std::future::pending().await,
                    }
                };
                let result = tokio::select! {
                    result = run(
                        executor.as_ref(),
                        &workspace,
                        &request,
                        Duration::MAX,
                        Some(&sink as &dyn OutputSink),
                    ) => result,
                    () = deadline => {
                        stopped.store(true, Ordering::SeqCst);
                        Err("Bash command stopped after reaching its background time limit".into())
                    }
                    () = over.wait() => Err("Command killed: output file exceeded 5GB".into()),
                };
                drop(sink);
                let _ = sender.send(Some(result));
            });
            let job = Arc::new(Job {
                abort: task.abort_handle(),
                finished,
                result: receiver,
                phase: AtomicPhase::new(),
                expired,
                capped,
                written,
                activity,
                id: id.clone(),
                command,
                label,
                call_id,
                owner,
                output,
                oom_kills,
                notice: Slot::default(),
            });
            self.jobs
                .entries
                .try_update(|entries| {
                    (!entries.closed).then(|| {
                        entries.jobs.insert(id.clone(), Arc::clone(&job));
                    })
                })
                .ok_or("Bash is unavailable because the agent is shutting down")?;
            admitted.fire();
            job
        };

        struct Admission(Option<tokio::task::AbortHandle>);
        impl Drop for Admission {
            fn drop(&mut self) {
                if let Some(abort) = self.0.take() {
                    abort.abort();
                }
            }
        }
        let mut admission = Admission(Some(job.abort.clone()));
        let timed_out = if background {
            let _ = arm.send(timeout);
            None
        } else {
            match tokio::time::timeout(timeout, completion(&job)).await {
                Ok(mut result) => {
                    self.jobs.forget(&id);
                    if let (Ok(result), Some(kills)) = (&mut result, job.memory_kills().await) {
                        let line = format!("[{kills}]\n");
                        let room = max_stderr.saturating_sub(line.len() + 1);
                        if result.stderr.len() > room {
                            result
                                .stderr
                                .truncate(result.stderr.floor_char_boundary(room));
                            result.truncated = true;
                        }
                        if !result.stderr.is_empty() && !result.stderr.ends_with('\n') {
                            result.stderr.push('\n');
                        }
                        result.stderr.push_str(&line);
                    }
                    return match result {
                        Ok(result) if result.truncated => Ok(kept(&job, result).await),
                        result => {
                            discard(&job);
                            result.map(BashExecution::Completed)
                        }
                    };
                }
                Err(_) if sleeps(&job.command) => {
                    job.abort.abort();
                    job.finished.wait().await;
                    self.jobs.forget(&id);
                    discard(&job);
                    return Err(format!(
                        "Bash command timed out after {}s",
                        timeout.as_secs_f64().round().max(1.0)
                    ));
                }
                Err(_) => {
                    let _ = arm.send(Duration::from_millis(DEFAULT_BACKGROUND_TIMEOUT_MS));
                    Some(timeout)
                }
            }
        };
        admission.0 = None;
        job.phase.advance(Phase::Background);
        let output_file = job.output.clone();
        let watch = self.jobs.watch(job);
        Ok(BashExecution::Background {
            message: started(&id, output_file.as_deref(), timed_out, watch),
            output_file: output_file.map(|path| path.display().to_string()),
            task_id: id,
        })
    }
}
async fn kept(job: &Job, result: BashResult) -> BashExecution {
    job.written.wait().await;
    let size = match &job.output {
        Some(path) => tokio::fs::metadata(path)
            .await
            .ok()
            .map(|metadata| (path.display().to_string(), metadata.len())),
        None => None,
    };
    match size {
        Some((output_file, output_bytes)) => BashExecution::Cut {
            result,
            output_file,
            output_bytes,
        },
        None => BashExecution::Completed(result),
    }
}
async fn completion(job: &Job) -> Outcome {
    settled(job.result.clone()).await
}
async fn settled(mut receiver: watch::Receiver<Option<Outcome>>) -> Outcome {
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return result;
        }
        if receiver.changed().await.is_err() {
            return Err("Bash task stopped".into());
        }
    }
}

pub(super) fn install(
    mut tools: ToolsBuilder,
    workspace: PathBuf,
    session: Option<SessionEnvironment>,
    jobs: Arc<Jobs>,
    process: Arc<dyn nanocode_process_contracts::ForegroundExecutor>,
) -> ToolsBuilder {
    let executor = LocalBash {
        workspace,
        jobs,
        executor: process,
    };
    tools = tools.tool(BashTool {
        bash: ClaudeBash::new(executor.clone()),
        jobs: executor.jobs.clone(),
        session,
    });
    tools.tool(TaskStopTool {
        jobs: executor.jobs,
    })
}
struct BashTool {
    bash: ClaudeBash<LocalBash>,
    jobs: Arc<Jobs>,
    session: Option<SessionEnvironment>,
}
impl Tool for BashTool {
    fn definition(&self) -> ToolDefinition {
        let schema = ClaudeBash::<LocalBash>::definitions().remove(0);
        ToolDefinition::function(
            "Bash",
            schema["description"].as_str().unwrap_or_default(),
            schema["input_schema"].clone(),
        )
        .with_output_schema(super::claude_output::schema("Bash"))
    }
    fn running_tasks(&self) -> Vec<RunningTask> {
        self.jobs.entries.read(|entries| {
            entries
                .jobs
                .values()
                .filter(|job| job.status() == "running")
                .map(|job| RunningTask {
                    id: job.id.clone(),
                    label: job.label.clone(),
                    silent_for: job.activity.silent_for(),
                })
                .collect()
        })
    }
    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let input = input.decode_json::<Value>()?;
                let owned = Owned {
                    jobs: &self.jobs,
                    call_id: context.call_id(),
                };
                self.jobs.owners.update(|owners| {
                    owners.insert(owned.call_id.to_owned(), context.session_id().to_owned());
                });
                Ok(
                    match self
                        .bash
                        .execute_call("Bash", input, self.session.clone(), Some(context.call_id()))
                        .await
                    {
                        Ok(output) => {
                            let value =
                                serde_json::from_str::<Value>(&output).unwrap_or(Value::Null);
                            let text = value["message"].as_str().map_or(output, str::to_owned);
                            ToolOutput::text(text).with_structured_result(value)
                        }
                        Err(error) => ToolOutput::error(error),
                    },
                )
            };
            output
        })
    }
}
struct Owned<'a> {
    jobs: &'a Jobs,
    call_id: &'a str,
}
impl Drop for Owned<'_> {
    fn drop(&mut self) {
        self.jobs.owners.update(|owners| {
            owners.remove(self.call_id);
        });
    }
}
pub(super) struct JobNotices(pub(super) Arc<Jobs>);
impl nanocode_tool_host::CodeModeToolPolicy for JobNotices {
    fn session_notices(&self, session_id: &str) -> Vec<String> {
        self.0.notices(session_id)
    }
    fn session_notice<'a>(
        &'a self,
        session_id: &'a str,
    ) -> nanocode_tool_host::embedded::HostFuture<'a, ()> {
        Box::pin(self.0.noticed(session_id))
    }
}
struct TaskStopTool {
    jobs: Arc<Jobs>,
}
impl Tool for TaskStopTool {
    fn definition(&self) -> ToolDefinition {
        let definition = crate::upstream::tool_value("TaskStop");
        ToolDefinition::function(
            "TaskStop",
            definition["description"]
                .as_str()
                .expect("upstream tool description"),
            definition["input_schema"].clone(),
        )
        .with_output_schema(super::claude_output::schema("TaskStop"))
    }
    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let input = input.decode_json::<Value>()?;
                Ok(match self.invoke(input, context).await {
                    Ok(value) => ToolOutput::text(value.to_string()).with_structured_result(value),
                    Err(error) => ToolOutput::error(error),
                })
            };
            output
        })
    }
}
impl TaskStopTool {
    async fn invoke(&self, input: Value, context: ToolContext<'_>) -> Result<Value, String> {
        let fields = input.as_object().ok_or("task input must be an object")?;
        if let Some(key) = fields
            .keys()
            .find(|key| !matches!(key.as_str(), "task_id" | "shell_id"))
        {
            return Err(format!("unsupported task option: {key}"));
        }
        let id = fields
            .get("task_id")
            .filter(|id| !id.is_null())
            .or_else(|| fields.get("shell_id"))
            .and_then(Value::as_str)
            .ok_or("task_id or its deprecated shell_id alias must be a string")?;
        let Some(job) = self
            .jobs
            .entries
            .read(|entries| entries.jobs.get(id).cloned())
        else {
            return self.stop_agent(id, context).await;
        };
        let status = job.status();
        if status != "running" {
            return Err(format!("Task {id} is not running (status: {status})"));
        }
        job.abort.abort();
        job.finished.wait().await;
        Ok(json!({
            "message": format!("Successfully stopped task: {id} ({})", job.command),
            "task_id": id,
            "task_type": "local_bash",
            "command": job.command,
        }))
    }
    async fn stop_agent(&self, id: &str, context: ToolContext<'_>) -> Result<Value, String> {
        let unknown = || "unknown task_id in this session".to_owned();
        let (Some(subagents), Some(after)) = (
            &self.jobs.subagents,
            id.parse::<u64>()
                .ok()
                .and_then(|agent| agent.checked_sub(1)),
        ) else {
            return Err(unknown());
        };
        let listed = delegate(
            &subagents.list,
            json!({"include_completed": true, "after_agent_id": after, "limit": 1}),
            context,
        )
        .await?;
        let agent = listed["agents"]
            .as_array()
            .and_then(|agents| agents.iter().find(|agent| agent["agent_id"] == after + 1))
            .ok_or_else(unknown)?;
        let status = agent["status"]["state"]
            .as_str()
            .ok_or("list_agents returned an agent without a status")?;
        if !matches!(status, "pending" | "running") {
            return Err(format!("Task {id} is not running (status: {status})"));
        }
        let role = agent["role"]
            .as_str()
            .ok_or("list_agents returned an agent without a role")?;
        delegate(&subagents.close, json!({"agent_id": after + 1}), context).await?;
        Ok(json!({
            "message": format!("Successfully stopped task: {id} ({role})"),
            "task_id": id,
            "task_type": "local_agent",
            "command": role,
        }))
    }
}
async fn delegate(
    tool: &Arc<dyn Tool>,
    input: Value,
    context: ToolContext<'_>,
) -> Result<Value, String> {
    let input = serde_json::value::to_raw_value(&input).map_err(|error| error.to_string())?;
    let mut output = tool
        .execute(ToolInput::Function(input), context)
        .await
        .map_err(|error| error.to_string())?;
    if !output.success {
        return Err(match output.output {
            nanocode_tool_host::contract::ToolOutputBody::Text(text) => text,
            body => body.structured_result().to_string(),
        });
    }
    Ok(output.take_structured_result())
}
