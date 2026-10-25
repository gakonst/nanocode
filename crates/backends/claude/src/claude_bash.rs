use crate::{
    agent::Steer,
    tools::{
        BashExecution, BashExecutor, BashRequest, BashResult, ClaudeBash,
        bash::DEFAULT_BACKGROUND_TIMEOUT_MS,
    },
};
use async_trait::async_trait;
use nanocode_agent_core::{Agent, PromptRequest, WeakAgent};
use nanocode_process_contracts::{ForegroundExecutor, LastActivity, OutputSink};
use nanocode_sync::{Atom, Signal};
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
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
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
const UNWATCHED: &str = "Nothing notifies you when the command finishes: read the output file that the result names to check on it. The file gets no line when the command ends, so if you need to know that it has, end the command with an `echo` of your own.";

type Outcome = Result<BashResult, String>;
struct Job {
    result: watch::Receiver<Option<Outcome>>,
    finished: Arc<Signal>,
    abort: tokio::task::AbortHandle,
    waiting: AtomicUsize,
    delivered: AtomicBool,
    backgrounded: AtomicBool,
    expired: Arc<AtomicBool>,
    capped: Arc<Signal>,
    written: Arc<Signal>,
    activity: Arc<LastActivity>,
    id: String,
    command: String,
    label: String,
    call_id: Option<String>,
    output: Option<PathBuf>,
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
    fn completion_notification(&self, outcome: &Outcome) -> String {
        if self.expired.load(Ordering::SeqCst) {
            return self.notification(
                Some("killed"),
                "was stopped after reaching its background time limit",
                &format!("\n<note>{DEADLINE_NOTE}</note>"),
                "",
            );
        }
        if self.capped.is_fired() {
            return self.notification(Some("killed"), "was stopped", "", "");
        }
        let (status, summary) = match outcome {
            Ok(result) if result.exit_code == 0 => {
                ("completed", "completed (exit code 0)".to_owned())
            }
            Ok(result) => (
                "failed",
                format!("failed with exit code {}", result.exit_code),
            ),
            Err(_) => ("failed", "failed".to_owned()),
        };
        self.notification(Some(status), &summary, "", "")
    }
}
struct Waiting<'a>(&'a Job);
impl<'a> Waiting<'a> {
    fn new(job: &'a Job) -> Self {
        job.waiting.fetch_add(1, Ordering::SeqCst);
        Self(job)
    }
}
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.waiting.fetch_sub(1, Ordering::SeqCst);
    }
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
    drop(tokio::spawn(async move {
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
    drop(tokio::spawn(async move {
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
fn sleeps(command: &str) -> bool {
    command
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || ";&|()".contains(ch))
        .next()
        == Some("sleep")
}
fn whole_duration(milliseconds: u64) -> String {
    let minutes = milliseconds / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}
#[derive(Clone, Copy)]
enum Watch {
    Notified,
    EndsWithTurn,
    Unwatched,
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
            Watch::Unwatched => UNWATCHED,
        }
        .to_owned(),
    );
    if matches!(watch, Watch::Notified) && timed_out.is_some() {
        parts.push(format!(
            "If it is still running after {} in the background, it will be stopped and you will be notified.",
            whole_duration(DEFAULT_BACKGROUND_TIMEOUT_MS)
        ));
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
}
impl Jobs {
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
            if job.backgrounded.load(Ordering::SeqCst) && job.status() == "running" {
                job.abort.abort();
            }
        }
    }
    fn watch(&self, job: Arc<Job>) -> Watch {
        let Some(binding) = self.agent.read(Clone::clone) else {
            return Watch::Unwatched;
        };
        let watch = if binding.ends_with_turn {
            Watch::EndsWithTurn
        } else {
            Watch::Notified
        };
        drop(tokio::spawn(async move {
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
                || job.waiting.load(Ordering::SeqCst) > 0
                || job.delivered.swap(true, Ordering::SeqCst)
            {
                return;
            }
            route(
                binding,
                job.id.clone(),
                job.completion_notification(&outcome),
            );
        }));
        watch
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
    drop(tokio::spawn(async move {
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
            let task = tokio::spawn(async move {
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
                waiting: AtomicUsize::new(0),
                delivered: AtomicBool::new(false),
                backgrounded: AtomicBool::new(false),
                expired,
                capped,
                written,
                activity,
                id: id.clone(),
                command,
                label,
                call_id,
                output,
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
                Ok(result) => {
                    self.jobs.forget(&id);
                    discard(&job);
                    return result.map(BashExecution::Completed);
                }
                Err(_) if sleeps(&job.command) => {
                    job.abort.abort();
                    job.finished.wait().await;
                    self.jobs.forget(&id);
                    discard(&job);
                    return Err("Bash command timed out".into());
                }
                Err(_) => {
                    let _ = arm.send(Duration::from_millis(DEFAULT_BACKGROUND_TIMEOUT_MS));
                    Some(timeout)
                }
            }
        };
        admission.0 = None;
        job.backgrounded.store(true, Ordering::SeqCst);
        let output_file = job.output.clone();
        let watch = self.jobs.watch(job);
        Ok(BashExecution::Background {
            message: started(&id, output_file.as_deref(), timed_out, watch),
            output_file: output_file.map(|path| path.display().to_string()),
            task_id: id,
        })
    }
}
async fn completion(job: &Job) -> Outcome {
    let mut receiver = job.result.clone();
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
    for name in ["TaskOutput", "TaskStop"] {
        tools = tools.tool(TaskTool {
            name,
            jobs: executor.jobs.clone(),
        });
    }
    tools
}
struct BashTool {
    bash: ClaudeBash<LocalBash>,
    jobs: Arc<Jobs>,
    session: Option<SessionEnvironment>,
}
#[async_trait]
impl Tool for BashTool {
    fn definition(&self) -> ToolDefinition {
        let schema = ClaudeBash::<LocalBash>::definitions().remove(0);
        ToolDefinition::function(
            "Bash",
            schema["description"].as_str().unwrap_or_default(),
            schema["input_schema"].clone(),
        )
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
    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let input = input.decode_json::<Value>()?;
        Ok(
            match self
                .bash
                .execute_call("Bash", input, self.session.clone(), Some(context.call_id()))
                .await
            {
                Ok(output) => {
                    let value = serde_json::from_str::<Value>(&output).unwrap_or(Value::Null);
                    let text = value["message"].as_str().map_or(output, str::to_owned);
                    ToolOutput::text(text).with_structured_result(value)
                }
                Err(error) => ToolOutput::error(error),
            },
        )
    }
}
struct TaskTool {
    name: &'static str,
    jobs: Arc<Jobs>,
}
#[async_trait]
impl Tool for TaskTool {
    fn definition(&self) -> ToolDefinition {
        if self.name == "TaskStop" {
            let definition = crate::upstream::tool_value(self.name);
            return ToolDefinition::function(
                self.name,
                definition["description"]
                    .as_str()
                    .expect("upstream tool description"),
                definition["input_schema"].clone(),
            );
        }
        ToolDefinition::function(
            self.name,
            "SDK extension: retrieve a session-owned Bash task's result, optionally waiting for completion.",
            json!({"type":"object","properties":{"task_id":{"type":"string"},"block":{"type":"boolean","default":true},"timeout":{"type":"integer","minimum":0,"maximum":300000,"default":30000}},"required":["task_id"],"additionalProperties":false}),
        )
    }
    async fn execute(&self, input: ToolInput, _context: ToolContext<'_>) -> ToolResult {
        let input = input.decode_json::<Value>()?;
        Ok(match self.invoke(input).await {
            Ok(value) => ToolOutput::text(value.to_string()).with_structured_result(value),
            Err(error) => ToolOutput::error(error),
        })
    }
}
impl TaskTool {
    async fn invoke(&self, input: Value) -> Result<Value, String> {
        let fields = input.as_object().ok_or("task input must be an object")?;
        for key in fields.keys() {
            if key != "task_id"
                && !(self.name == "TaskStop" && key == "shell_id")
                && !(self.name == "TaskOutput" && matches!(key.as_str(), "block" | "timeout"))
            {
                return Err(format!("unsupported task option: {key}"));
            }
        }
        if self.name == "TaskStop"
            && fields.contains_key("task_id")
            && fields.contains_key("shell_id")
        {
            return Err("provide task_id or its deprecated shell_id alias, not both".into());
        }
        let id = fields
            .get("task_id")
            .or_else(|| {
                if self.name == "TaskStop" {
                    fields.get("shell_id")
                } else {
                    None
                }
            })
            .and_then(Value::as_str)
            .ok_or("task_id or its deprecated shell_id alias must be a string")?;
        let job = self
            .jobs
            .entries
            .read(|entries| entries.jobs.get(id).cloned())
            .ok_or("unknown task_id in this session")?;
        if self.name == "TaskStop" {
            let status = job.status();
            if status != "running" {
                return Err(format!("Task {id} is not running (status: {status})"));
            }
            job.abort.abort();
            job.finished.wait().await;
            return Ok(json!({
                "message": format!("Successfully stopped task: {id} ({})", job.command),
                "task_id": id,
                "task_type": "local_bash",
                "command": job.command,
            }));
        }
        let block = input
            .get("block")
            .map_or(Some(true), Value::as_bool)
            .ok_or("block must be boolean")?;
        let timeout = input
            .get("timeout")
            .map_or(Some(30000), Value::as_u64)
            .filter(|v| *v <= 300000)
            .ok_or("timeout must be 0..300000 milliseconds")?;
        let waiting = block.then(|| Waiting::new(&job));
        let result = if block {
            tokio::time::timeout(Duration::from_millis(timeout), completion(&job))
                .await
                .ok()
        } else {
            job.result.borrow().clone().or_else(|| {
                job.abort
                    .is_finished()
                    .then(|| Err("Bash task stopped".into()))
            })
        };
        if result.is_some() {
            job.delivered.store(true, Ordering::SeqCst);
        }
        drop(waiting);
        Ok(match result {
            None => json!({"task_id":id,"status":"running"}),
            Some(Err(error)) => json!({"task_id":id,"status":"error","error":error}),
            Some(Ok(result)) => {
                json!({"task_id":id,"status":"completed","stdout":result.stdout,"stderr":result.stderr,"exit_code":result.exit_code,"truncated":result.truncated})
            }
        })
    }
}
