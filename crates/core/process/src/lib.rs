use nanocode_home::SessionEnvironment;
use serde::Serialize;
use std::{ffi::OsString, future::Future, path::Path, pin::Pin, sync::Arc, time::Duration};

#[cfg(not(target_family = "wasm"))]
pub type ProcessFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(target_family = "wasm")]
pub type ProcessFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub struct ExecCommand {
    pub script: String,
    pub workdir: Option<String>,
    pub shell: Option<String>,
    pub login: Option<bool>,
    pub tty: bool,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub caller: Option<String>,
}

impl ExecCommand {
    pub const fn new(
        script: String,
        workdir: Option<String>,
        shell: Option<String>,
        login: Option<bool>,
        tty: bool,
        yield_time_ms: Option<u64>,
        max_output_tokens: Option<usize>,
    ) -> Self {
        Self {
            script,
            workdir,
            shell,
            login,
            tty,
            yield_time_ms,
            max_output_tokens,
            caller: None,
        }
    }

    #[must_use]
    pub fn from_caller(mut self, caller: impl Into<String>) -> Self {
        self.caller = Some(caller.into());
        self
    }
}

pub struct WriteStdin {
    pub session_id: i32,
    pub chars: String,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub caller: Option<String>,
}

impl WriteStdin {
    pub const fn new(
        session_id: i32,
        chars: String,
        yield_time_ms: Option<u64>,
        max_output_tokens: Option<usize>,
    ) -> Self {
        Self {
            session_id,
            chars,
            yield_time_ms,
            max_output_tokens,
            caller: None,
        }
    }

    #[must_use]
    pub fn from_caller(mut self, caller: impl Into<String>) -> Self {
        self.caller = Some(caller.into());
        self
    }
}

#[derive(Serialize)]
pub struct ExecCommandResult {
    #[serde(skip)]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
    pub wall_time_seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_token_count: Option<usize>,
    pub output: String,
}

pub struct ForegroundOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub truncated: bool,
}

pub trait ShellExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        command: ExecCommand,
        workspace: &'a Path,
    ) -> ProcessFuture<'a, ExecCommandResult>;
    fn write_stdin(&self, request: WriteStdin) -> ProcessFuture<'_, ExecCommandResult>;
    fn terminate_all(&self) -> ProcessFuture<'_, ()>;
}

pub trait OutputSink: Send + Sync {
    fn write(&self, bytes: &[u8]);
}

pub struct ForegroundCommand<'a> {
    pub workspace: &'a Path,
    pub command: &'a str,
    pub timeout: Duration,
    pub max_stdout: usize,
    pub max_stderr: usize,
    pub session: Option<&'a SessionEnvironment>,
    pub output: Option<&'a dyn OutputSink>,
}

pub trait ForegroundExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        command: ForegroundCommand<'a>,
    ) -> ProcessFuture<'a, Result<ForegroundOutput, String>>;
}

pub trait FileHost: Send + Sync {
    fn read<'a>(&'a self, path: &'a Path) -> ProcessFuture<'a, Result<Option<String>, String>>;
    fn write<'a>(
        &'a self,
        path: &'a Path,
        contents: &'a str,
    ) -> ProcessFuture<'a, Result<(), String>>;
    fn remove<'a>(&'a self, path: &'a Path) -> ProcessFuture<'a, Result<(), String>>;
}

pub trait ProcessExecutors: Send + Sync {
    fn shell(&self, environment: Vec<(OsString, OsString)>) -> Arc<dyn ShellExecutor>;
    fn foreground(&self) -> Arc<dyn ForegroundExecutor>;
}
impl ExecCommandResult {
    pub fn failed(wall_time: Duration, output: String) -> Self {
        Self {
            error: Some(output.clone()),
            chunk_id: None,
            wall_time_seconds: wall_time.as_secs_f64(),
            exit_code: Some(1),
            session_id: None,
            original_token_count: None,
            output,
        }
    }
}

pub const DEFAULT_EXEC_YIELD_MS: u64 = 10_000;
pub const DEFAULT_WRITE_YIELD_MS: u64 = 250;
pub const DEFAULT_POLL_YIELD_MS: u64 = 5_000;
pub const MAX_EXEC_YIELD_MS: u64 = 30_000;
pub const MAX_POLL_YIELD_MS: u64 = 300_000;

const BYTES_PER_TOKEN: usize = 4;
const REDACTION: &str = "[REDACTED]";

pub fn effective_token_limit(requested: Option<usize>) -> usize {
    requested.map_or(usize::MAX, |value| value.saturating_mul(BYTES_PER_TOKEN))
}

/// Secret values shorter than this are not redacted: replacing a one- to three-byte value such
/// as `1` or `on` everywhere would garble all output while hiding nothing a guess would not
/// find. This is the rule tempoxyz/pyramid applied to its per-thread secrets before redaction
/// moved here.
pub const MIN_REDACTED_SECRET_BYTES: usize = 4;

/// Replaces every secret value of at least [`MIN_REDACTED_SECRET_BYTES`] bytes in `output`
/// with `[REDACTED]`, longest first, then truncates it to `limit` bytes; the flag says whether
/// it was truncated.
pub fn redact_and_limit(mut output: String, secrets: &[String], limit: usize) -> (String, bool) {
    let mut secrets = secrets
        .iter()
        .filter(|secret| secret.len() >= MIN_REDACTED_SECRET_BYTES)
        .collect::<Vec<_>>();
    secrets.sort_unstable_by_key(|secret| std::cmp::Reverse(secret.len()));
    for secret in secrets {
        output = output.replace(secret.as_str(), REDACTION);
    }
    if output.len() <= limit {
        return (output, false);
    }

    let original_token_count = output.len().saturating_add(3) / BYTES_PER_TOKEN;
    let total_lines = output.lines().count();
    let left_budget = limit / 2;
    let right_budget = limit.saturating_sub(left_budget);
    let prefix_end = output.floor_char_boundary(left_budget);
    let suffix_start = output
        .ceil_char_boundary(output.len().saturating_sub(right_budget))
        .max(prefix_end);
    let removed_tokens = output.len().saturating_sub(limit).saturating_add(3) / BYTES_PER_TOKEN;
    let truncated = format!(
        "Warning: truncated output (original token count: {original_token_count})\nTotal output lines: {total_lines}\n\n{}…{removed_tokens} tokens truncated…{}",
        &output[..prefix_end],
        &output[suffix_start..],
    );
    (truncated, true)
}
