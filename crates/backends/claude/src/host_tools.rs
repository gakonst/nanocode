use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::tools::{
    BashExecution, BashExecutor, BashRequest, BashResult, ClaudeBash, host::HostTool, text_files,
};
use futures_util::future::{AbortHandle, abortable};
use nanocode_process_contracts::{FileHost, ForegroundCommand, ForegroundExecutor};
use nanocode_sync::{Atom, Signal};
use nanocode_tool_host::{
    Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult, Tools, ToolsBuildError,
    embedded::HostFuture,
};
use serde_json::{Value, json};

const BACKGROUND_NOTE: &str = "Nothing notifies you when it finishes, and it stops if this session's host shuts down, which can happen once your turn ends: check on it with TaskOutput (with block: true it waits up to timeout milliseconds) and stop it with TaskStop, and wait for it before your final response if you need its result.";

#[cfg(not(target_family = "wasm"))]
fn spawn(future: impl std::future::Future<Output = ()> + Send + 'static) {
    drop(tokio::spawn(future));
}

#[cfg(target_family = "wasm")]
fn spawn(future: impl std::future::Future<Output = ()> + 'static) {
    nanocode_platform::spawn_local(future);
}

pub fn host_tools(
    workspace: PathBuf,
    files: Arc<dyn FileHost>,
    shell: Arc<dyn ForegroundExecutor>,
) -> Result<Tools, ToolsBuildError> {
    let workspace: Arc<Path> = workspace.into();
    let jobs = Arc::new(HostJobs::default());
    let mut tools = file_tools(Arc::clone(&workspace), files);
    tools = tools.tool(HostBashTool(ClaudeBash::new(HostBash {
        workspace,
        shell,
        jobs: Arc::clone(&jobs),
    })));
    for tool in [HostTool::TaskOutput, HostTool::TaskStop] {
        tools = tools.tool(HostTaskTool {
            tool,
            jobs: Arc::clone(&jobs),
        });
    }
    tools
        .build()
        .map(|tools| tools.with_code_mode_policy(crate::code_mode_policy::claude_tool_policy()))
}

pub fn host_file_tools(
    workspace: PathBuf,
    files: Arc<dyn FileHost>,
) -> Result<Tools, ToolsBuildError> {
    file_tools(workspace.into(), files)
        .build()
        .map(|tools| tools.with_code_mode_policy(crate::code_mode_policy::claude_tool_policy()))
}

fn file_tools(workspace: Arc<Path>, files: Arc<dyn FileHost>) -> nanocode_tool_host::ToolsBuilder {
    let mut tools = Tools::builder().without_defaults();
    for schema in text_files::definitions() {
        let name = schema["name"].as_str().unwrap_or_default().to_owned();
        tools = tools.tool(HostFileTool {
            definition: ToolDefinition::function(
                name.clone(),
                schema["description"].as_str().unwrap_or_default(),
                schema["input_schema"].clone(),
            ),
            name,
            workspace: Arc::clone(&workspace),
            files: Arc::clone(&files),
        });
    }
    tools
}

struct HostFileTool {
    name: String,
    definition: ToolDefinition,
    workspace: Arc<Path>,
    files: Arc<dyn FileHost>,
}

impl HostFileTool {
    async fn run(&self, input: &Value, budget: usize) -> Result<String, String> {
        text_files::check(&self.name, input)?;
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .ok_or("missing or invalid file_path")?;
        let path = self.workspace.join(file_path);
        match self.name.as_str() {
            "Read" => {
                if input.get("pages").is_some() {
                    return Err("pages is only applicable to PDF files".into());
                }
                let content = self.existing(&path).await?;
                text_files::read(file_path, content.len() as u64, input, budget, || {
                    Ok(content.lines().map(|line| Ok(line.to_owned())))
                })
            }
            "Write" => {
                let content = input
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or("missing or invalid content")?;
                self.files.write(&path, content).await?;
                Ok(format!("Wrote {}", path.display()))
            }
            _ => {
                let edit = text_files::Edit::parse(input)?;
                let content = self.existing(&path).await?;
                let (updated, count) = edit.apply(&content)?;
                self.files.write(&path, &updated).await?;
                Ok(format!(
                    "Edited {} ({count} replacement(s))",
                    path.display()
                ))
            }
        }
    }

    async fn existing(&self, path: &Path) -> Result<String, String> {
        self.files
            .read(path)
            .await?
            .ok_or_else(|| format!("path: {} does not exist", path.display()))
    }
}

impl Tool for HostFileTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> HostFuture<'async_trait, ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let input: Value = input.decode_json()?;
            Ok(
                match self.run(&input, context.output_token_budget()).await {
                    Ok(text) => ToolOutput::text(text),
                    Err(error) => ToolOutput::error(error),
                },
            )
        })
    }
}

struct HostBash {
    workspace: Arc<Path>,
    shell: Arc<dyn ForegroundExecutor>,
    jobs: Arc<HostJobs>,
}

type Outcome = Result<BashResult, String>;

#[derive(Default)]
struct HostJobs {
    next: AtomicU64,
    entries: Atom<BTreeMap<String, Arc<HostJob>>>,
}

struct HostJob {
    command: String,
    result: Atom<Option<Outcome>>,
    finished: Signal,
    stopped: AtomicBool,
    abort: AbortHandle,
}

impl HostJob {
    fn status(&self) -> &'static str {
        match &*self.result.get() {
            _ if self.stopped.load(Ordering::SeqCst) => "killed",
            Some(Ok(result)) if result.exit_code == 0 => "completed",
            Some(_) => "failed",
            None => "running",
        }
    }
}

async fn run(shell: &dyn ForegroundExecutor, workspace: &Path, request: &BashRequest) -> Outcome {
    let output = shell
        .execute(ForegroundCommand {
            workspace,
            command: &request.command,
            timeout: Duration::from_millis(request.timeout_ms),
            max_stdout: request.max_stdout_bytes,
            max_stderr: request.max_stderr_bytes,
            session: request.session.as_ref(),
            output: None,
        })
        .await?;
    Ok(BashResult {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: output.exit_code,
        truncated: output.truncated,
    })
}

impl BashExecutor for HostBash {
    async fn execute(&self, request: BashRequest) -> Outcome {
        run(self.shell.as_ref(), &self.workspace, &request).await
    }

    async fn execute_with_background(
        &self,
        request: BashRequest,
        background: bool,
    ) -> Result<BashExecution, String> {
        if !background {
            return self.execute(request).await.map(BashExecution::Completed);
        }
        let id = format!(
            "bash-{}",
            self.jobs.next.fetch_add(1, Ordering::Relaxed) + 1
        );
        let shell = Arc::clone(&self.shell);
        let workspace = Arc::clone(&self.workspace);
        let command = request.command.clone();
        let (task, abort) =
            abortable(async move { run(shell.as_ref(), &workspace, &request).await });
        let job = Arc::new(HostJob {
            command,
            result: Atom::new(None),
            finished: Signal::new(),
            stopped: AtomicBool::new(false),
            abort,
        });
        self.jobs.entries.update(|entries| {
            entries.insert(id.clone(), Arc::clone(&job));
        });
        spawn(async move {
            let outcome = task
                .await
                .unwrap_or_else(|_| Err("Bash task stopped with TaskStop".into()));
            job.result.set(Some(outcome));
            job.finished.fire();
        });
        Ok(BashExecution::Background {
            message: format!("Command running in background with ID: {id}. {BACKGROUND_NOTE}"),
            output_file: None,
            task_id: id,
        })
    }
}

struct HostTaskTool {
    tool: HostTool,
    jobs: Arc<HostJobs>,
}

impl HostTaskTool {
    async fn invoke(&self, input: Value) -> Result<Value, String> {
        let stop = matches!(self.tool, HostTool::TaskStop);
        let fields = input.as_object().ok_or("task input must be an object")?;
        for key in fields.keys() {
            if key != "task_id"
                && !(stop && key == "shell_id")
                && !(!stop && matches!(key.as_str(), "block" | "timeout"))
            {
                return Err(format!("unsupported task option: {key}"));
            }
        }
        if stop && fields.contains_key("task_id") && fields.contains_key("shell_id") {
            return Err("provide task_id or its deprecated shell_id alias, not both".into());
        }
        let id = fields
            .get("task_id")
            .or_else(|| stop.then(|| fields.get("shell_id")).flatten())
            .and_then(Value::as_str)
            .ok_or("task_id or its deprecated shell_id alias must be a string")?;
        let job = self
            .jobs
            .entries
            .read(|entries| entries.get(id).cloned())
            .ok_or("unknown task_id in this session")?;
        if stop {
            let status = job.status();
            if status != "running" {
                return Err(format!("Task {id} is not running (status: {status})"));
            }
            job.stopped.store(true, Ordering::SeqCst);
            job.abort.abort();
            job.finished.wait().await;
            return Ok(json!({
                "message": format!("Successfully stopped task: {id} ({})", job.command),
                "task_id": id,
                "task_type": "local_bash",
                "command": job.command,
            }));
        }
        let block = fields
            .get("block")
            .map_or(Some(true), Value::as_bool)
            .ok_or("block must be boolean")?;
        let timeout = fields
            .get("timeout")
            .map_or(Some(30_000), Value::as_u64)
            .filter(|value| *value <= 300_000)
            .ok_or("timeout must be 0..300000 milliseconds")?;
        if block {
            let _ = nanocode_platform::timeout(Duration::from_millis(timeout), job.finished.wait())
                .await;
        }
        Ok(match &*job.result.get() {
            None => json!({"task_id": id, "status": "running"}),
            Some(Err(error)) => json!({"task_id": id, "status": job.status(), "error": error}),
            Some(Ok(result)) => json!({
                "task_id": id,
                "status": job.status(),
                "stdout": result.stdout,
                "stderr": result.stderr,
                "exit_code": result.exit_code,
                "truncated": result.truncated,
            }),
        })
    }
}

impl Tool for HostTaskTool {
    fn definition(&self) -> ToolDefinition {
        let name = self.tool.name();
        if matches!(self.tool, HostTool::TaskStop) {
            let definition = nanocode_backend_claude_tools::upstream::tool_value(name);
            return ToolDefinition::function(
                name,
                definition["description"].as_str().unwrap_or_default(),
                definition["input_schema"].clone(),
            );
        }
        ToolDefinition::function(
            name,
            "SDK extension: retrieve a background Bash task's result, optionally waiting for completion.",
            json!({"type":"object","properties":{"task_id":{"type":"string"},"block":{"type":"boolean","default":true},"timeout":{"type":"integer","minimum":0,"maximum":300000,"default":30000}},"required":["task_id"],"additionalProperties":false}),
        )
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        _context: ToolContext<'life1>,
    ) -> HostFuture<'async_trait, ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let input = input.decode_json::<Value>()?;
            Ok(match self.invoke(input).await {
                Ok(value) => ToolOutput::text(value.to_string()).with_structured_result(value),
                Err(error) => ToolOutput::error(error),
            })
        })
    }
}

struct HostBashTool(ClaudeBash<HostBash>);

impl Tool for HostBashTool {
    fn definition(&self) -> ToolDefinition {
        let schema = ClaudeBash::<HostBash>::definitions().remove(0);
        ToolDefinition::function(
            "Bash",
            schema["description"].as_str().unwrap_or_default(),
            schema["input_schema"].clone(),
        )
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> HostFuture<'async_trait, ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let input = input.decode_json::<Value>()?;
            Ok(
                match self
                    .0
                    .execute_call("Bash", input, None, Some(context.call_id()))
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
        })
    }
}
