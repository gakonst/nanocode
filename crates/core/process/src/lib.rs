use nanocode_home::SessionEnvironment;
use serde::Serialize;
use std::{
    ffi::OsString,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime},
};

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
    fn running(&self) -> Vec<RunningProcess> {
        Vec::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningProcess {
    pub session_id: i64,
    pub caller: Option<String>,
    pub command: String,
    pub workdir: PathBuf,
    pub started_at: SystemTime,
    pub in_call: bool,
    pub silent_for: Duration,
}

#[cfg(not(target_family = "wasm"))]
pub struct LastActivity(std::sync::atomic::AtomicU64);

#[cfg(not(target_family = "wasm"))]
impl LastActivity {
    #[must_use]
    pub fn new() -> Self {
        Self(std::sync::atomic::AtomicU64::new(Self::clock()))
    }

    pub fn touch(&self) {
        self.0
            .fetch_max(Self::clock(), std::sync::atomic::Ordering::Relaxed);
    }

    #[must_use]
    pub fn silent_for(&self) -> Duration {
        Duration::from_micros(
            Self::clock().saturating_sub(self.0.load(std::sync::atomic::Ordering::Relaxed)),
        )
    }

    fn clock() -> u64 {
        static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        u64::try_from(
            ORIGIN
                .get_or_init(std::time::Instant::now)
                .elapsed()
                .as_micros(),
        )
        .unwrap_or(u64::MAX)
    }
}

#[cfg(not(target_family = "wasm"))]
impl Default for LastActivity {
    fn default() -> Self {
        Self::new()
    }
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

pub fn effective_token_limit(requested: Option<usize>) -> usize {
    requested.map_or(usize::MAX, |value| value.saturating_mul(BYTES_PER_TOKEN))
}

pub const MIN_REDACTED_SECRET_BYTES: usize = 4;

pub fn redact_and_limit(mut output: String, secrets: &[String], limit: usize) -> (String, bool) {
    Redactor::raw(MIN_REDACTED_SECRET_BYTES)
        .values(secrets)
        .text(&mut output);
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

pub const REDACTED: &str = "[REDACTED]";

#[derive(Clone, Debug, Default)]
pub struct Redactor {
    min_bytes: usize,
    raw: bool,
    forms: Vec<(String, String)>,
}

impl Redactor {
    pub const fn new(min_bytes: usize) -> Self {
        Self {
            min_bytes,
            raw: false,
            forms: Vec::new(),
        }
    }

    pub const fn raw(min_bytes: usize) -> Self {
        Self {
            min_bytes,
            raw: true,
            forms: Vec::new(),
        }
    }

    pub fn add(&mut self, value: &str, placeholder: &str) -> bool {
        if value.is_empty() || value.len() < self.min_bytes {
            return false;
        }
        let escaped = (!self.raw).then(|| json_escape(value));
        let mut added = false;
        for form in std::iter::once(value).chain(escaped.as_deref()) {
            if !self.forms.iter().any(|(known, _)| known == form) {
                self.forms.push((form.to_owned(), placeholder.to_owned()));
                added = true;
            }
        }
        self.forms
            .sort_by_key(|(form, _)| std::cmp::Reverse(form.len()));
        added
    }

    #[must_use]
    pub fn named<N: AsRef<str>, V: AsRef<str>>(
        mut self,
        secrets: impl IntoIterator<Item = (N, V)>,
    ) -> Self {
        for (name, value) in secrets {
            self.add(value.as_ref(), &format!("[{}]", name.as_ref()));
        }
        self
    }

    #[must_use]
    pub fn values<V: AsRef<str>>(mut self, values: impl IntoIterator<Item = V>) -> Self {
        for value in values {
            self.add(value.as_ref(), REDACTED);
        }
        self
    }

    pub fn forms(&self) -> impl Iterator<Item = (&str, &str)> {
        self.forms
            .iter()
            .map(|(form, placeholder)| (form.as_str(), placeholder.as_str()))
    }

    pub fn text(&self, text: &mut String) -> bool {
        let mut found = false;
        for (form, placeholder) in &self.forms {
            if text.contains(form.as_str()) {
                *text = text.replace(form.as_str(), placeholder);
                found = true;
            }
        }
        found
    }

    pub fn json(&self, value: &mut serde_json::Value) -> bool {
        use serde_json::Value;
        match value {
            Value::String(text) => self.text(text),
            Value::Array(items) => items
                .iter_mut()
                .fold(false, |found, item| self.json(item) | found),
            Value::Object(fields) => fields
                .values_mut()
                .fold(false, |found, item| self.json(item) | found),
            _ => false,
        }
    }

    pub fn json_text(&self, stored: &str) -> Option<String> {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(stored) {
            return self
                .json(&mut value)
                .then(|| serde_json::to_string(&value).ok())
                .flatten();
        }
        let mut stored = stored.to_owned();
        self.text(&mut stored).then_some(stored)
    }
}

pub fn json_escape(value: &str) -> String {
    let quoted = serde_json::Value::from(value).to_string();
    quoted[1..quoted.len() - 1].to_owned()
}
