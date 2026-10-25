use std::{path::PathBuf, sync::Arc};

use nanocode_tools::{RunningTask, ToolDefinition};
use serde::Deserialize;

use super::{StandardTool, Tool, ToolContext, ToolInput, ToolOutput, ToolResult};

use nanocode_process_contracts::{ExecCommand, ExecCommandResult, ShellExecutor, WriteStdin};

pub(crate) struct ExecCommandHandler {
    workspace: PathBuf,
    sessions: Arc<dyn ShellExecutor>,
}

impl ExecCommandHandler {
    pub(crate) const fn new(workspace: PathBuf, sessions: Arc<dyn ShellExecutor>) -> Self {
        Self {
            workspace,
            sessions,
        }
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]
impl Tool for ExecCommandHandler {
    fn definition(&self) -> ToolDefinition {
        StandardTool::ExecCommand.definition()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn running_tasks(&self) -> Vec<RunningTask> {
        self.sessions
            .running()
            .into_iter()
            .map(|process| RunningTask {
                id: process.session_id.to_string(),
                label: process.command,
                silent_for: process.silent_for,
            })
            .collect()
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let arguments = input.decode_json::<ExecCommandArguments>()?;
        let command = ExecCommand::new(
            arguments.cmd,
            arguments.workdir,
            arguments.shell,
            arguments.login,
            arguments.tty,
            arguments.yield_time_ms,
            arguments.max_output_tokens,
        )
        .from_caller(context.session_id());
        let result = self.sessions.execute(command, &self.workspace).await;
        Ok(shell_execution(&result))
    }
}

pub(crate) struct WriteStdinHandler {
    sessions: Arc<dyn ShellExecutor>,
}

impl WriteStdinHandler {
    pub(crate) const fn new(sessions: Arc<dyn ShellExecutor>) -> Self {
        Self { sessions }
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]
impl Tool for WriteStdinHandler {
    fn definition(&self) -> ToolDefinition {
        StandardTool::WriteStdin.definition()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let arguments = input.decode_json::<WriteStdinArguments>()?;
        let request = WriteStdin::new(
            arguments.session_id,
            arguments.chars,
            arguments.yield_time_ms,
            arguments.max_output_tokens,
        )
        .from_caller(context.session_id());
        let result = self.sessions.write_stdin(request).await;
        Ok(shell_execution(&result))
    }
}

fn shell_execution(result: &ExecCommandResult) -> ToolOutput {
    if let Some(error) = &result.error {
        return ToolOutput::error(error);
    }
    let structured_result = match serde_json::to_value(result) {
        Ok(value) => value,
        Err(error) => {
            return ToolOutput::error(format!("failed to encode tool result: {error}"));
        }
    };
    ToolOutput::text(shell_response_text(result))
        .with_structured_result(structured_result)
        .with_process_trace(
            result.exit_code,
            result.session_id,
            result.original_token_count,
            result.output.len(),
            result.wall_time_seconds,
        )
}

fn shell_response_text(result: &ExecCommandResult) -> String {
    let mut sections = Vec::new();
    if let Some(chunk_id) = result
        .chunk_id
        .as_deref()
        .filter(|chunk_id| !chunk_id.is_empty())
    {
        sections.push(format!("Chunk ID: {chunk_id}"));
    }
    sections.push(format!(
        "Wall time: {:.4} seconds",
        result.wall_time_seconds
    ));
    if let Some(exit_code) = result.exit_code {
        sections.push(format!("Process exited with code {exit_code}"));
    }
    if let Some(session_id) = result.session_id {
        sections.push(format!("Process running with session ID {session_id}"));
    }
    if let Some(original_token_count) = result.original_token_count {
        sections.push(format!("Original token count: {original_token_count}"));
    }
    sections.push("Output:".to_owned());
    sections.push(result.output.clone());
    sections.join("\n")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecCommandArguments {
    cmd: String,

    #[serde(default, rename = "justification")]
    _justification: Option<String>,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    login: Option<bool>,
    #[serde(default)]
    tty: bool,
    #[serde(default)]
    yield_time_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default, rename = "prefix_rule")]
    _prefix_rule: Option<Vec<String>>,
    #[serde(default, rename = "sandbox_permissions")]
    _sandbox_permissions: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteStdinArguments {
    session_id: i32,
    #[serde(default)]
    chars: String,
    #[serde(default)]
    yield_time_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
}
