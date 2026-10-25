#![cfg(not(target_family = "wasm"))]

#[cfg(unix)]
pub mod browser;
pub mod driver;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use nanocode_tool_contracts::{
    Tool, ToolContext, ToolDefinition, ToolError, ToolInput, ToolOutput, ToolOutputContent,
    ToolResult,
};
use nanocode_types::ImageDetail;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    hash::{Hash as _, Hasher as _},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
};

const PROVIDER_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const INTERRUPTED: &str = "CUA session interrupted by caller cancellation or transport failure. Upstream/native input may still be running and effects are uncertain; do not replay uncertain input. Call cua_repl.js_reset, then inspect the surface before continuing. Reset does not prove earlier input stopped.";
const PINNED_CATALOG: &str = include_str!("catalog.json");
const CUA_DRIVER_PROVIDER: &str = include_str!("cua_driver.mjs");
const CUA_DRIVER_CATALOG: &str = include_str!("cua_driver_catalog.json");
const CUA_DRIVER_SEARCH: &[&str] = &[
    ".local/bin/cua-driver",
    "/usr/local/bin/cua-driver",
    "/opt/homebrew/bin/cua-driver",
    "/Applications/CuaDriver.app/Contents/MacOS/cua-driver",
];
const FORK_DRIVER_ARGS: &str = "mcp --direct";
const NODE_SEARCH: &[&str] = &["/opt/homebrew/bin/node", "/usr/local/bin/node"];
const CUA_REPL_SETTINGS: &[&str] = &[
    "CUA_REPL_SETTLE_MS",
    "CUA_REPL_LAUNCH_TIMEOUT_MS",
    "CUA_REPL_BROWSER_WAIT_MS",
    "CUA_REPL_PAGE_SETTLE_MS",
    "CUA_DRIVER_PERMISSION_MODE",
];
const CONFIRMATION_POLICY: &str = "No confirmation policy applies.";
const PROVIDER_ENVIRONMENT: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TEMP",
    "SystemRoot",
    "LOCALAPPDATA",
    "DISPLAY",
    "XAUTHORITY",
    "WAYLAND_DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "LANG",
    "SKY_ENABLE_AUDIO",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CatalogCheck {
    #[default]
    Exact,
    Names,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutcomeUnknown {
    TimedOut(Duration),
    Transport(String),
}

impl std::fmt::Display for OutcomeUnknown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut(timeout) => write!(
                formatter,
                "CUA provider did not finish this call within {} seconds and was stopped; whether the call took effect is unknown",
                timeout.as_secs()
            ),
            Self::Transport(error) => write!(
                formatter,
                "CUA provider failed during this call ({error}) and was stopped; whether the call took effect is unknown"
            ),
        }
    }
}

impl std::error::Error for OutcomeUnknown {}

#[derive(Clone, Debug)]
pub struct ComputerConfig {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub environment: BTreeMap<OsString, OsString>,
    provider_catalog: Option<Vec<ProviderTool>>,
    catalog_check: CatalogCheck,
    startup_timeout: Duration,
    step_timeout: Option<Duration>,
    restart_after_failure: bool,
    required: Vec<PathBuf>,
}

impl ComputerConfig {
    pub fn installed() -> Result<Self, ToolError> {
        if !cfg!(target_os = "macos") {
            return Err(
                "Default Sky CUA requires macOS; supply an explicit ComputerTools connection"
                    .into(),
            );
        }
        let home = std::env::var_os("HOME").ok_or("Default Sky CUA requires HOME")?;
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&home).join(".codex"));
        let base = PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/cua_node");
        let launcher = base.join("lib/node_modules/@oai/cua-repl/bin/cua-repl.mjs");
        let node_repl = base.join("bin/node_repl");
        let mut config = Self::new(base.join("bin/node"));
        config.args = vec![launcher.clone().into_os_string()];
        let service = codex_home.join("computer-use/Codex Computer Use.app");
        config.environment = [
            (
                "CUA_REPL_NODE_REPL_PATH",
                node_repl.clone().into_os_string(),
            ),
            (
                "CUA_REPL_ENABLED_SURFACES",
                OsString::from("browser,computer"),
            ),
            (
                "NODE_REPL_NODE_PATH",
                base.join("bin/node").into_os_string(),
            ),
            (
                "NODE_REPL_NODE_MODULE_DIRS",
                base.join("lib/node_modules").into_os_string(),
            ),
            (
                "NODE_REPL_TRUSTED_CODE_PATHS",
                base.join("lib/node_modules").into_os_string(),
            ),
            ("SKY_CUA_SERVICE_PATH", service.clone().into_os_string()),
            ("CODEX_HOME", codex_home.clone().into_os_string()),
        ]
        .into_iter()
        .map(|(key, value)| (OsString::from(key), value))
        .collect();
        config.required = vec![config.executable.clone(), launcher, node_repl, service];
        config.provider_catalog = Some(serde_json::from_str(PINNED_CATALOG)?);
        Ok(config)
    }

    pub fn cua_driver() -> Result<Self, ToolError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("cua-driver CUA requires HOME")?;
        let node = std::env::var_os("CUA_REPL_NODE")
            .map(PathBuf::from)
            .or_else(|| program("node"))
            .or_else(|| {
                NODE_SEARCH
                    .iter()
                    .map(PathBuf::from)
                    .find(|path| executable(path))
            })
            .ok_or("cua-driver CUA needs Node.js 22 or newer: install node or set CUA_REPL_NODE")?;
        let fork = fork_driver(&home).filter(|path| executable(path));
        let explicit = std::env::var_os("CUA_DRIVER_PATH").map(PathBuf::from);
        let direct = explicit.is_none() && fork.is_some();
        let driver = explicit
            .or(fork)
            .or_else(|| program("cua-driver"))
            .or_else(|| {
                CUA_DRIVER_SEARCH
                    .iter()
                    .map(|path| home.join(path))
                    .find(|path| executable(path))
            })
            .ok_or(
                "cua-driver is not installed: run nanocode cua install, or set CUA_DRIVER_PATH",
            )?;
        let source = CUA_DRIVER_PROVIDER
            .replacen("__CATALOG__", CUA_DRIVER_CATALOG.trim(), 1)
            .replacen("__VERSION__", env!("CARGO_PKG_VERSION"), 1);
        let script = provider_script(&home, &source)?;
        let mut config = Self::new(node);
        config.args = vec![
            OsString::from("--expose-internals"),
            script.into_os_string(),
        ];
        config.environment =
            [
                ("CUA_DRIVER_PATH", driver.clone().into_os_string()),
                (
                    "CUA_REPL_BROWSER_SOCKET",
                    browser_socket(&home).into_os_string(),
                ),
            ]
            .into_iter()
            .map(|(key, value)| (OsString::from(key), value))
            .chain(direct.then(|| {
                (
                    OsString::from("CUA_DRIVER_ARGS"),
                    OsString::from(FORK_DRIVER_ARGS),
                )
            }))
            .chain(CUA_REPL_SETTINGS.iter().filter_map(|name| {
                std::env::var_os(name).map(|value| (OsString::from(name), value))
            }))
            .collect();
        config.required = vec![config.executable.clone(), driver];
        config.provider_catalog = Some(serde_json::from_str(CUA_DRIVER_CATALOG)?);
        Ok(config)
    }

    pub fn detect() -> Result<Self, ToolError> {
        match std::env::var("NANOCODE_CUA").as_deref() {
            Ok("cua-driver") => Self::cua_driver(),
            Ok("sky") => Self::installed(),
            Ok(other) => {
                Err(format!("NANOCODE_CUA must be cua-driver or sky, not {other:?}").into())
            }
            Err(_) => Self::cua_driver(),
        }
    }

    pub fn unavailable(&self) -> Option<String> {
        self.required
            .iter()
            .find(|path| !path.exists())
            .map(|path| {
                format!(
                    "Default Sky CUA installation missing: expected {}; install ChatGPT.app with Codex Computer Use",
                    path.display()
                )
            })
    }

    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            environment: BTreeMap::new(),
            provider_catalog: None,
            catalog_check: CatalogCheck::Exact,
            startup_timeout: PROVIDER_STARTUP_TIMEOUT,
            step_timeout: None,
            restart_after_failure: false,
            required: Vec::new(),
        }
    }

    pub fn mcp(executable: impl Into<PathBuf>) -> Self {
        Self::new(executable)
    }

    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    #[must_use]
    pub fn inherit_env_prefix(mut self, prefix: &str) -> Self {
        self.environment.extend(
            std::env::vars_os()
                .filter(|(name, _)| name.as_encoded_bytes().starts_with(prefix.as_bytes())),
        );
        self
    }

    #[must_use]
    pub fn catalog(mut self, catalog: Vec<ProviderTool>, check: CatalogCheck) -> Self {
        self.provider_catalog = Some(catalog);
        self.catalog_check = check;
        self
    }

    #[must_use]
    pub fn startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    #[must_use]
    pub fn step_timeout(mut self, timeout: Duration) -> Self {
        self.step_timeout = Some(timeout);
        self
    }

    #[must_use]
    pub fn restart_after_failure(mut self) -> Self {
        self.restart_after_failure = true;
        self
    }
}

fn nanocode_home(home: &std::path::Path) -> PathBuf {
    std::env::var_os("NANOCODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".nanocode"))
}

pub fn fork_driver(home: &std::path::Path) -> Option<PathBuf> {
    cfg!(target_os = "macos").then(|| {
        std::env::var_os("NANOCODE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".nanocode"))
            .join("cua-driver/cua-driver")
    })
}

pub fn browser_socket(home: &std::path::Path) -> PathBuf {
    nanocode_home(home).join("browser/browser.sock")
}

fn provider_script(home: &std::path::Path, source: &str) -> Result<PathBuf, ToolError> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    let directory = nanocode_home(home).join("cua");
    let path = directory.join(format!("cua-driver-{:016x}.mjs", hasher.finish()));
    if std::fs::read(&path).is_ok_and(|bytes| bytes == source.as_bytes()) {
        return Ok(path);
    }
    let failed = |error: std::io::Error| {
        ToolError::from(format!(
            "cannot write the cua-driver provider to {}: {error}",
            path.display()
        ))
    };
    std::fs::create_dir_all(&directory).map_err(failed)?;
    let temporary = directory.join(format!(".cua-driver-{}.tmp", std::process::id()));
    std::fs::write(&temporary, source).map_err(failed)?;
    std::fs::rename(&temporary, &path).map_err(failed)?;
    Ok(path)
}

fn program(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|path| executable(path))
}

#[cfg(unix)]
fn executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[async_trait]
pub trait ComputerExecutor: Send + Sync + 'static {
    async fn end_turn(
        &self,
        _session_id: &str,
        _turn_id: &str,
        _hook_event_name: &str,
    ) -> Result<(), ToolError> {
        Ok(())
    }

    async fn invoke_tool(
        &self,
        name: &str,
        arguments: Value,
        context: ToolContext<'_>,
    ) -> ToolResult;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProviderTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

impl ProviderTool {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            metadata: BTreeMap::new(),
            input_schema: json!({"type": "object"}),
        }
    }

    pub fn model_visible(&self) -> bool {
        self.metadata
            .get("_meta")
            .and_then(|meta| meta.pointer("/ui/visibility"))
            .and_then(Value::as_array)
            .is_none_or(|visibility| visibility.iter().any(|value| value == "model"))
    }
}

#[derive(Clone)]
pub struct ComputerTools {
    executor: Arc<dyn ComputerExecutor>,
    catalog: Arc<Vec<ProviderTool>>,
}
impl ComputerTools {
    pub async fn connect(mut config: ComputerConfig) -> Result<Self, ToolError> {
        if config.provider_catalog.is_none() {
            config.provider_catalog = Some(Process::start(&config).await?.catalog);
        }
        Ok(Self::local(config))
    }
    fn local(config: ComputerConfig) -> Self {
        let catalog = Arc::new(
            config
                .provider_catalog
                .clone()
                .expect("connect pins or discovers the provider catalog before registration"),
        );
        let (dispatch, requests) = mpsc::unbounded_channel();
        tokio::spawn(route_sessions(config, requests));
        Self {
            executor: Arc::new(LocalComputer { dispatch }),
            catalog,
        }
    }

    pub fn new(executor: impl ComputerExecutor, catalog: Vec<ProviderTool>) -> Self {
        Self {
            executor: Arc::new(executor),
            catalog: Arc::new(catalog),
        }
    }

    pub fn catalog(&self) -> &[ProviderTool] {
        &self.catalog
    }

    pub fn tools(&self) -> impl Iterator<Item = ComputerTool> + '_ {
        self.catalog
            .iter()
            .filter(|definition| definition.model_visible())
            .cloned()
            .map(|definition| ComputerTool {
                executor: self.executor.clone(),
                definition,
            })
    }
    pub fn tool(&self, name: &str) -> Option<ComputerTool> {
        self.catalog
            .iter()
            .find(|definition| definition.name == name)
            .cloned()
            .map(|definition| ComputerTool {
                executor: self.executor.clone(),
                definition,
            })
    }

    pub async fn end_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        hook_event_name: &str,
    ) -> Result<(), ToolError> {
        self.executor
            .end_turn(session_id, turn_id, hook_event_name)
            .await
    }
    pub fn js(&self) -> ComputerTool {
        self.tool("js").expect("CUA provider does not publish js")
    }
    pub fn reset(&self) -> ComputerTool {
        self.tool("js_reset")
            .expect("CUA provider does not publish js_reset")
    }
}

#[derive(Clone)]
pub struct ComputerTool {
    executor: Arc<dyn ComputerExecutor>,
    definition: ProviderTool,
}

impl ComputerTool {
    pub fn provider_definition(&self) -> &ProviderTool {
        &self.definition
    }
}

#[async_trait]
impl Tool for ComputerTool {
    async fn end_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        hook_event_name: &str,
    ) -> Result<(), ToolError> {
        self.executor
            .end_turn(session_id, turn_id, hook_event_name)
            .await
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }
    fn definition(&self) -> ToolDefinition {
        let definition = ToolDefinition::function(
            format!("mcp__cua_repl__{}", self.definition.name),
            self.definition.description.as_deref().unwrap_or(""),
            self.definition.input_schema.clone(),
        );
        match self.definition.metadata.get("outputSchema") {
            Some(schema) => definition.with_output_schema(schema.clone()),
            None => definition,
        }
    }
    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        self.executor
            .invoke_tool(
                &self.definition.name,
                input.decode_json::<Value>()?,
                context,
            )
            .await
    }
}

struct LocalComputer {
    dispatch: mpsc::UnboundedSender<SessionRequest>,
}

struct SessionRequest {
    lifecycle: bool,
    session: String,
    turn_id: Option<String>,
    call_id: String,
    model: String,
    name: String,
    arguments: Value,
    response: oneshot::Sender<ToolResult>,
}

#[async_trait]
impl ComputerExecutor for LocalComputer {
    async fn end_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        hook_event_name: &str,
    ) -> Result<(), ToolError> {
        if !matches!(hook_event_name, "Stop" | "Interrupt" | "SubagentStop") {
            return Err("Invalid CUA turn lifecycle event".into());
        }
        let (response, result) = oneshot::channel();
        self.dispatch
            .send(SessionRequest {
                lifecycle: true,
                session: session_id.into(),
                turn_id: Some(turn_id.into()),
                call_id: String::new(),
                model: String::new(),
                name: "turn_ended".into(),
                arguments: json!({"hook_event_name":hook_event_name,"session_id":session_id,"turn_id":turn_id}),
                response,
            })
            .map_err(|_| "CUA attachment is closed")?;
        let output = result.await.map_err(|_| "CUA attachment is closed")??;
        if !output.success {
            return Err("CUA turn_ended failed; cleanup was not retried".into());
        }
        Ok(())
    }

    async fn invoke_tool(
        &self,
        name: &str,
        arguments: Value,
        context: ToolContext<'_>,
    ) -> ToolResult {
        let session = context.session_id().to_owned();
        let (response, result) = oneshot::channel();
        self.dispatch
            .send(SessionRequest {
                lifecycle: false,
                session,
                turn_id: context.turn_id().map(str::to_owned),
                call_id: context.call_id().to_owned(),
                model: context.model().to_owned(),
                name: name.into(),
                arguments,
                response,
            })
            .map_err(|_| "CUA attachment is closed")?;
        result.await.map_err(|_| "CUA attachment is closed")?
    }
}

async fn route_sessions(
    config: ComputerConfig,
    mut requests: mpsc::UnboundedReceiver<SessionRequest>,
) {
    let mut sessions = BTreeMap::<String, mpsc::UnboundedSender<SessionRequest>>::new();
    while let Some(request) = requests.recv().await {
        let session = request.session.clone();
        if request.lifecycle && !sessions.contains_key(&session) {
            let _ = request.response.send(Ok(ToolOutput::text("")));
            continue;
        }
        let owner = sessions
            .entry(session.clone())
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::unbounded_channel();
                tokio::spawn(run_session(config.clone(), session.clone(), receiver));
                sender
            })
            .clone();
        if let Err(error) = owner.send(request) {
            if error.0.lifecycle {
                let _ = error.0.response.send(Ok(ToolOutput::text("")));
                sessions.remove(&session);
                continue;
            }
            let (sender, receiver) = mpsc::unbounded_channel();
            tokio::spawn(run_session(config.clone(), session.clone(), receiver));
            let _ = sender.send(error.0);
            sessions.insert(session, sender);
        }
    }
}

fn call_meta(session: &str, turn: Option<&str>, call: &str, model: &str) -> Value {
    json!({
        "x-codex-turn-metadata": turn_metadata(session, turn, call, model),
        "openai/confirmation_policies": {"computer_use": CONFIRMATION_POLICY, "browser_use": CONFIRMATION_POLICY},
    })
}

fn turn_metadata(session: &str, turn: Option<&str>, call: &str, model: &str) -> Value {
    let mut metadata =
        json!({"session_id":session,"thread_id":session,"call_id":call,"model":model});
    if let Some(turn) = turn.filter(|turn| !turn.trim().is_empty()) {
        metadata["turn_id"] = json!(turn);
    }
    metadata
}

async fn run_session(
    config: ComputerConfig,
    session: String,
    mut requests: mpsc::UnboundedReceiver<SessionRequest>,
) {
    let mut process = None;
    let mut interrupted = false;
    let mut turns = BTreeMap::<String, (String, String)>::new();
    while let Some(request) = requests.recv().await {
        let SessionRequest {
            lifecycle,
            turn_id,
            mut call_id,
            mut model,
            name,
            arguments,
            mut response,
            ..
        } = request;

        if response.is_closed() {
            continue;
        }
        if lifecycle {
            let retained = turn_id.as_ref().and_then(|turn| turns.remove(turn));
            let Some((previous_call, previous_model)) = retained else {
                let _ = response.send(Ok(ToolOutput::text("")));
                continue;
            };
            let hook = config
                .provider_catalog
                .as_ref()
                .and_then(|catalog| catalog.iter().find(|tool| tool.name == "turn_ended"));
            if process.is_none() || interrupted || hook.is_none() {
                let _ = response.send(Ok(ToolOutput::text("")));
                continue;
            }
            call_id = previous_call;
            model = previous_model;
        }
        if interrupted && name != "js_reset" {
            let _ = response.send(Err(INTERRUPTED.into()));
            continue;
        }

        let previous = process.take();
        if !lifecycle && let Some(turn) = &turn_id {
            turns.insert(turn.clone(), (call_id.clone(), model.clone()));
        }
        let execution = async {
            let mut process = match previous {
                Some(process) => process,
                None => Process::start(&config).await.map_err(Failure::Unsent)?,
            };
            let call = process.rpc(
                "tools/call",
                json!({"name":name,"arguments":arguments,
                "_meta": call_meta(&session, turn_id.as_deref(), &call_id, &model)}),
            );
            let reply = match config.step_timeout {
                Some(timeout) => tokio::time::timeout(timeout, call)
                    .await
                    .map_err(|_| Failure::Unknown(Box::new(OutcomeUnknown::TimedOut(timeout))))?,
                None => call.await,
            };
            let value = match reply {
                Ok(value) => value,
                Err(RpcFailure::Reply(error)) => {
                    return Err(Failure::Answered(Box::new(process), error));
                }
                Err(RpcFailure::Transport(error)) => {
                    let unknown = OutcomeUnknown::Transport(error.to_string());
                    return Err(Failure::Unknown(Box::new(unknown)));
                }
            };
            let output = output(value).map_err(Failure::Unknown)?;
            Ok::<_, Failure>((process, output))
        };
        let outcome = tokio::select! {
            biased;
            () = response.closed() => {
                turns.clear();
                interrupted = !config.restart_after_failure;
                continue;
            }
            outcome = execution => outcome,
        };
        match outcome {
            Ok((owned, output)) => {
                process = Some(owned);
                if name == "js_reset" && output.success {
                    interrupted = false;
                }
                if response.send(Ok(output)).is_err() {
                    process = None;
                    turns.clear();
                    interrupted = !config.restart_after_failure;
                }
            }
            Err(Failure::Answered(owned, error)) => {
                process = Some(*owned);
                let _ = response.send(Err(error));
            }
            Err(Failure::Unsent(error)) => {
                turns.clear();
                let _ = response.send(Err(error));
            }
            Err(Failure::Unknown(error)) => {
                turns.clear();
                interrupted |= !config.restart_after_failure;
                let _ = response.send(Err(error));
            }
        }
    }
}

enum Failure {
    Unsent(ToolError),
    Answered(Box<Process>, ToolError),
    Unknown(ToolError),
}

enum RpcFailure {
    Reply(ToolError),
    Transport(ToolError),
}

impl From<RpcFailure> for ToolError {
    fn from(failure: RpcFailure) -> Self {
        match failure {
            RpcFailure::Reply(error) | RpcFailure::Transport(error) => error,
        }
    }
}

struct Process {
    _child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    catalog: Vec<ProviderTool>,
}
impl Process {
    async fn start(config: &ComputerConfig) -> Result<Self, ToolError> {
        tokio::time::timeout(config.startup_timeout, Self::start_and_discover(config))
            .await
            .map_err(|_| format!("CUA provider startup timed out after {} seconds during initialization or catalog discovery; owned transport discarded. Upstream/native work may continue.", config.startup_timeout.as_secs()))?
    }

    async fn start_and_discover(config: &ComputerConfig) -> Result<Self, ToolError> {
        if let Some(missing) = config.unavailable() {
            return Err(missing.into());
        }
        let mut command = Command::new(&config.executable);
        command.args(&config.args).env_clear();

        for name in PROVIDER_ENVIRONMENT {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.envs(&config.environment);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            format!(
                "Cannot start CUA provider {}: {error}. Check the provider installation.",
                config.executable.display()
            )
        })?;
        let input = child.stdin.take().ok_or("CUA stdin unavailable")?;
        let output = BufReader::new(child.stdout.take().ok_or("CUA stdout unavailable")?);
        let mut process = Self {
            _child: child,
            input,
            output,
            next_id: 0,
            catalog: Vec::new(),
        };
        process.rpc("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{"elicitation":{"form":{}}},"clientInfo":{"name":"nanocode-cua","version":env!("CARGO_PKG_VERSION")}})).await?;
        process
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await?;
        let catalog = process.discover().await?;
        if let Some(expected) = &config.provider_catalog {
            let changed = match config.catalog_check {
                CatalogCheck::Exact => expected
                    .iter()
                    .chain(&catalog)
                    .map(|tool| tool.name.as_str())
                    .filter(|name| {
                        expected.iter().find(|tool| tool.name == *name)
                            != catalog.iter().find(|tool| tool.name == *name)
                    })
                    .collect::<BTreeSet<_>>(),
                CatalogCheck::Names => expected
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .filter(|name| !catalog.iter().any(|tool| tool.name == *name))
                    .collect(),
            };
            if !changed.is_empty() {
                return Err(format!(
                    "CUA provider tools differ from the definitions advertised to the model: {}. Nothing was dispatched; calls are refused until the advertised catalog matches this provider version",
                    changed.into_iter().collect::<Vec<_>>().join(", ")
                )
                .into());
            }
        }
        process.catalog = catalog;
        Ok(process)
    }
    async fn discover(&mut self) -> Result<Vec<ProviderTool>, ToolError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut cursors = BTreeSet::new();
        loop {
            let page = self
                .rpc(
                    "tools/list",
                    cursor
                        .as_ref()
                        .map_or_else(|| json!({}), |cursor| json!({"cursor": cursor})),
                )
                .await?;
            tools.extend(
                page["tools"]
                    .as_array()
                    .ok_or("CUA provider did not return an MCP tools/list catalog")?
                    .iter()
                    .cloned(),
            );
            match page.get("nextCursor") {
                None => break,
                Some(value) => {
                    let next = value
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .ok_or("CUA provider returned an invalid tools/list cursor")?;
                    if !cursors.insert(next.to_owned()) {
                        return Err("CUA provider returned a repeated tools/list cursor".into());
                    }
                    cursor = Some(next.to_owned());
                }
            }
        }
        let catalog: Vec<ProviderTool> = tools
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?;
        validate_catalog(&catalog)?;
        Ok(catalog)
    }
    async fn send(&mut self, value: Value) -> Result<(), ToolError> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        Ok(())
    }
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, RpcFailure> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
            .map_err(RpcFailure::Transport)?;
        let mut line = Vec::new();
        loop {
            let read = self.output.read_until(b'\n', &mut line).await;
            if read.map_err(|error| RpcFailure::Transport(error.into()))? == 0 {
                return Err(RpcFailure::Transport(
                    "CUA runtime closed its output".into(),
                ));
            }
            let value: Value = serde_json::from_slice(&line)
                .map_err(|error| RpcFailure::Transport(error.into()))?;
            line.clear();
            if let Some(method) = value.get("method") {
                if let Some(request_id) = value.get("id") {
                    self.send(match method.as_str() {
                        Some("elicitation/create") => json!({"jsonrpc":"2.0","id":request_id,"result":elicitation(&value["params"])}),
                        _ => json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"Unsupported provider server request"}}),
                    }).await.map_err(RpcFailure::Transport)?;
                }
                continue;
            }
            if value["id"] != id {
                return Err(RpcFailure::Transport(
                    "CUA response belongs to a different call".into(),
                ));
            }
            if let Some(error) = value.get("error") {
                return Err(RpcFailure::Reply(
                    format!(
                        "CUA: {}",
                        error["message"].as_str().unwrap_or("runtime error")
                    )
                    .into(),
                ));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| RpcFailure::Transport("CUA response is missing its result".into()));
        }
    }
}

fn elicitation(params: &Value) -> Value {
    let approval = params["mode"].as_str().is_none_or(|mode| mode == "form")
        && params["requestedSchema"]["properties"]
            .as_object()
            .is_some_and(serde_json::Map::is_empty);
    if approval {
        json!({"action":"accept","content":{}})
    } else {
        json!({"action":"decline"})
    }
}

fn validate_catalog(catalog: &[ProviderTool]) -> Result<(), ToolError> {
    let mut names = BTreeSet::new();
    for tool in catalog {
        if tool.name.is_empty() || !names.insert(&tool.name) {
            return Err("CUA provider tool names must be non-empty and unique".into());
        }
        if !tool.input_schema.is_object() {
            return Err("CUA provider tool inputSchema must be a JSON schema object".into());
        }
    }
    Ok(())
}

pub fn output(value: Value) -> ToolResult {
    let mut content = Vec::new();
    for item in value["content"]
        .as_array()
        .ok_or("CUA result has no content array")?
    {
        match item["type"].as_str() {
            Some("text") => content.push(ToolOutputContent::InputText {
                text: item["text"].as_str().ok_or("Invalid CUA text")?.into(),
            }),
            Some("image") => {
                let mime = item["mimeType"].as_str().ok_or("Invalid CUA image type")?;
                if !matches!(mime, "image/png" | "image/jpeg" | "image/webp") {
                    return Err("Unsupported CUA image type".into());
                }
                let data = item["data"].as_str().ok_or("Invalid CUA image")?;
                base64_simd::STANDARD
                    .check(data.as_bytes())
                    .or_else(|_| STANDARD.decode(data).map(|_| ()))?;
                let header = &data.as_bytes()[..data.len().min(16)];
                let mut bytes = [0; 12];
                let bytes = base64_simd::STANDARD
                    .decode(header, base64_simd::Out::from_slice(&mut bytes))?;
                let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    "image/png"
                } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
                    "image/jpeg"
                } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
                    "image/webp"
                } else {
                    return Err("Invalid CUA image bytes".into());
                };
                content.push(ToolOutputContent::InputImage {
                    image_url: format!("data:{mime};base64,{data}"),
                    detail: ImageDetail::Original,
                });
            }
            Some("audio") => {
                let mime = item["mimeType"].as_str().ok_or("Invalid CUA audio type")?;
                if !matches!(mime, "audio/wav" | "audio/mpeg") {
                    return Err("Unsupported CUA audio type".into());
                }
                let data = item["data"].as_str().ok_or("Invalid CUA audio")?;
                base64_simd::STANDARD
                    .check(data.as_bytes())
                    .or_else(|_| STANDARD.decode(data).map(|_| ()))?;
                content.push(ToolOutputContent::InputAudio {
                    audio_url: format!("data:{mime};base64,{data}"),
                });
            }
            _ => return Err("Unsupported CUA output content".into()),
        }
    }
    let success = value["isError"] != true;
    let metadata = value.get("_meta").cloned();
    let mut output = ToolOutput::content(content).with_structured_result(value);
    if let Some(metadata) = metadata {
        output = output.with_metadata(metadata);
    }
    output.success = success;
    Ok(output)
}
