use crate::tools::SessionEnvironment;
use serde_json::{Value, json};

pub const MAX_COMMAND_BYTES: usize = 1024 * 1024;

pub const MAX_DESCRIPTION_BYTES: usize = 1024;

pub const MAX_STREAM_BYTES: usize = 30_000;

pub const NESTED_STREAM_BYTES: usize = 128 * 1024;

pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const BLOCKED_SLEEP_SECONDS: f64 = 25.0;

pub const MAX_TIMEOUT_MS: u64 = 600_000;

pub const DEFAULT_BACKGROUND_TIMEOUT_MS: u64 = 600_000;

pub const MAX_BACKGROUND_TIMEOUT_MS: u64 = 7_200_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BashRequest {
    pub command: String,

    pub description: Option<String>,

    pub call_id: Option<String>,

    pub timeout_ms: u64,

    pub max_stdout_bytes: usize,

    pub max_stderr_bytes: usize,

    pub session: Option<SessionEnvironment>,
}

impl BashRequest {
    #[cfg(not(target_family = "wasm"))]
    pub fn apply_session(&self, command: &mut std::process::Command) {
        SessionEnvironment::apply_or_clear(self.session.as_ref(), command);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BashResult {
    pub stdout: String,

    pub stderr: String,

    pub exit_code: i32,

    pub truncated: bool,
}

pub trait BashFuture<T>: Future<Output = T> + nanocode_platform::MaybeSend {}
impl<T, F: Future<Output = T> + nanocode_platform::MaybeSend> BashFuture<T> for F {}

pub trait SandboxBashExecutor: Send + Sync {
    fn execute(&self, request: BashRequest) -> impl BashFuture<Result<BashResult, String>>;
}

pub enum BashExecution {
    Completed(BashResult),

    Cut {
        result: BashResult,
        output_file: String,
        output_bytes: u64,
    },

    Background {
        task_id: String,
        output_file: Option<String>,
        message: String,
    },
}

pub trait BashExecutor: Send + Sync {
    fn nested_timeout_ms(&self) -> u64 {
        DEFAULT_TIMEOUT_MS
    }

    fn execute_with_background(
        &self,
        request: BashRequest,
        background: bool,
    ) -> impl BashFuture<Result<BashExecution, String>> {
        async move {
            if background {
                return Err("this executor does not support background commands".into());
            }
            self.execute(request).await.map(BashExecution::Completed)
        }
    }

    fn execute(&self, request: BashRequest) -> impl BashFuture<Result<BashResult, String>>;
}

impl<E: SandboxBashExecutor> BashExecutor for E {
    async fn execute(&self, request: BashRequest) -> Result<BashResult, String> {
        SandboxBashExecutor::execute(self, request).await
    }
}

pub struct ClaudeBash<E: BashExecutor> {
    executor: E,
}

impl<E: BashExecutor> ClaudeBash<E> {
    pub const fn new(executor: E) -> Self {
        Self { executor }
    }

    #[must_use]
    pub fn definitions() -> Vec<Value> {
        ["Bash"]
            .into_iter()
            .map(crate::upstream::tool_value)
            .collect()
    }

    pub async fn execute(&self, name: &str, input: Value) -> Result<String, String> {
        self.execute_in_session(name, input, None).await
    }

    pub async fn execute_in_session(
        &self,
        name: &str,
        input: Value,
        session: Option<SessionEnvironment>,
    ) -> Result<String, String> {
        self.execute_call(name, input, session, None).await
    }

    pub async fn execute_call(
        &self,
        name: &str,
        input: Value,
        session: Option<SessionEnvironment>,
        call_id: Option<&str>,
    ) -> Result<String, String> {
        if name != "Bash" {
            return Err(format!("unknown Claude Bash tool: {name}"));
        }
        let fields = input.as_object().ok_or("Bash input must be an object")?;
        for key in fields.keys() {
            if !matches!(
                key.as_str(),
                "command"
                    | "description"
                    | "timeout"
                    | "run_in_background"
                    | "dangerouslyDisableSandbox"
            ) {
                return Err(format!("unsupported Bash option: {key}"));
            }
        }

        if fields
            .get("dangerouslyDisableSandbox")
            .map_or(Some(false), Value::as_bool)
            .ok_or("invalid dangerouslyDisableSandbox")?
        {
            return Err("this host does not authorize disabling the sandbox".into());
        }
        let background = fields
            .get("run_in_background")
            .map_or(Some(false), Value::as_bool)
            .ok_or("invalid run_in_background")?;
        let command = fields
            .get("command")
            .and_then(Value::as_str)
            .ok_or("missing or invalid command")?;
        if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES {
            return Err(format!(
                "command must be nonblank and at most {MAX_COMMAND_BYTES} bytes"
            ));
        }
        if !background && let Some(sleep) = leading_sleep(command) {
            return Err(format!(
                "Blocked: {sleep}. To wait for a command you started, use run_in_background: true. Do not chain shorter sleeps to work around this block."
            ));
        }
        let description = match fields.get("description") {
            None => None,
            Some(value) => {
                let text = value.as_str().ok_or("invalid description")?;
                if text.len() > MAX_DESCRIPTION_BYTES {
                    return Err(format!("description exceeds {MAX_DESCRIPTION_BYTES} bytes"));
                }
                Some(text.to_owned())
            }
        };
        let nested = call_id.is_some_and(|id| id.contains("/code-"));
        let (default, maximum) = if background {
            (DEFAULT_BACKGROUND_TIMEOUT_MS, MAX_BACKGROUND_TIMEOUT_MS)
        } else if nested {
            (self.executor.nested_timeout_ms(), MAX_TIMEOUT_MS)
        } else {
            (DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS)
        };
        let requested = fields
            .get("timeout")
            .map_or(Some(default), Value::as_u64)
            .ok_or("invalid timeout")?;
        if requested == 0 {
            return Err("timeout must be at least 1 millisecond".into());
        }
        let timeout_ms = requested.min(maximum);
        let reduced = (requested > maximum).then(|| {
            format!(
                "The call asked for a timeout of {requested} ms, more than the maximum of {maximum} ms, so the command had the maximum."
            )
        });
        let stream_bytes = if nested {
            NESTED_STREAM_BYTES
        } else {
            MAX_STREAM_BYTES
        };
        let request = BashRequest {
            command: command.to_owned(),
            description,
            call_id: call_id.map(str::to_owned),
            timeout_ms,
            max_stdout_bytes: stream_bytes,
            max_stderr_bytes: stream_bytes,
            session,
        };
        let result = self
            .executor
            .execute_with_background(request, background)
            .await
            .map_err(|e| {
                let (bounded, _) = cap_utf8(&e, MAX_STREAM_BYTES);
                format!("command executor: {bounded}")
            })?;
        let (result, kept) = match result {
            BashExecution::Completed(result) => (result, None),
            BashExecution::Cut {
                result,
                output_file,
                output_bytes,
            } => (result, Some((output_file, output_bytes))),
            BashExecution::Background {
                task_id,
                output_file,
                message,
            } => {
                let message = match reduced {
                    Some(reduced) => format!("{message} {reduced}"),
                    None => message,
                };
                return Ok(json!({
                    "stdout": format!("{message}\n"),
                    "stderr": "",
                    "exit_code": null,
                    "interrupted": false,
                    "truncated": false,
                    "background": true,
                    "status": "running",
                    "task_id": task_id,
                    "output_file": output_file,
                    "output_bytes": null,
                    "message": message,
                })
                .to_string());
            }
        };
        let kept = kept.as_ref().map(|(file, bytes)| (file.as_str(), *bytes));
        let (stdout, stderr, truncated) = bounded_output(&result, stream_bytes, kept);
        Ok(json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": result.exit_code,
            "interrupted": false,
            "truncated": truncated,
            "background": false,
            "status": "completed",
            "task_id": null,
            "output_file": kept.map(|(file, _)| file),
            "output_bytes": kept.map(|(_, bytes)| bytes),
            "message": reduced,
        })
        .to_string())
    }
}

#[must_use]
pub fn bounded_output(
    result: &BashResult,
    stream_bytes: usize,
    kept: Option<(&str, u64)>,
) -> (String, String, bool) {
    let (stdout, stdout_cut) = cap_utf8(&result.stdout, stream_bytes);
    let (stderr, stderr_cut) = cap_utf8(&result.stderr, stream_bytes);
    let truncated = result.truncated || stdout_cut || stderr_cut;
    let stderr_cut = stderr_cut || (result.truncated && stderr.len() >= stream_bytes);
    let stdout_cut = stdout_cut || (truncated && (!stderr_cut || stdout.len() >= stream_bytes));
    (
        with_cut_notice("stdout", stdout, stdout_cut, kept),
        with_cut_notice("stderr", stderr, stderr_cut, kept),
        truncated,
    )
}

fn with_cut_notice(stream: &str, text: &str, cut: bool, kept: Option<(&str, u64)>) -> String {
    if !cut {
        return text.to_owned();
    }
    let rest = match kept {
        Some((file, bytes)) => format!(
            "the command printed {bytes} bytes in all, stdout and stderr together, and {file} holds them"
        ),
        None => {
            "the rest was not kept, so send the command's output to a file and read it in parts"
                .to_owned()
        }
    };
    let line = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    format!(
        "{text}{line}[{stream} cut after its first {} bytes: {rest}]\n",
        text.len()
    )
}

fn leading_sleep(command: &str) -> Option<String> {
    let command = command.trim();
    let end = command.find(['&', '|', ';', '\n']).unwrap_or(command.len());
    let seconds = command[..end].trim().strip_prefix("sleep")?;
    if !seconds.starts_with(char::is_whitespace) {
        return None;
    }
    let seconds = seconds.trim();
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let seconds: f64 = seconds.parse().ok()?;
    if seconds < BLOCKED_SLEEP_SECONDS {
        return None;
    }
    let rest = command[end..]
        .trim_start_matches(['&', '|', ';', '\n', ' ', '\t'])
        .trim();
    Some(if rest.is_empty() {
        format!("standalone sleep {seconds}")
    } else {
        format!("sleep {seconds} followed by: {rest}")
    })
}

fn cap_utf8(value: &str, max: usize) -> (&str, bool) {
    if value.len() <= max {
        return (value, false);
    }
    (&value[..value.floor_char_boundary(max)], true)
}
