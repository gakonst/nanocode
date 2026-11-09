use std::path::{Path, PathBuf};

use nanocode::{
    Claude, ClaudeHarness, Codex, CodexHarness, Model, ModelSelection, Nanocode, Runner,
    RunnerServerConfig, ServiceTier, Thinking,
    agent::{Agent, AgentEvents},
    claude::ClaudeClient,
    durability::SessionStore,
};

use crate::{
    cli::{RemoteArgs, RunnerModeArg},
    gateways::{self, Gateway, Routes},
};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

type WarmUp = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
pub(crate) type Notices = std::pin::Pin<Box<dyn futures_util::Stream<Item = String> + Send>>;

#[derive(Clone, Default)]
pub struct ModelClients {
    openai: Option<std::sync::Arc<dyn Fn() -> nanocode::codex::OpenAiBuilder + Send + Sync>>,
    claude: Option<std::sync::Arc<dyn Fn() -> ClaudeClient + Send + Sync>>,
    warm_up: Option<std::sync::Arc<dyn Fn() -> WarmUp + Send + Sync>>,
    status: Option<std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    notices: Option<std::sync::Arc<dyn Fn() -> Notices + Send + Sync>>,
    panel: Option<PanelSource>,
    tool_environment: Vec<(String, String)>,
    mcp_servers: Vec<(String, nanocode::tools::mcp::McpServer)>,
    models: Option<std::sync::Arc<[Model]>>,
    gateways: Vec<Gateway>,
    preferred: Option<String>,
}

#[derive(Clone)]
struct PanelSource {
    command: &'static str,
    about: &'static str,
    panel: std::sync::Arc<dyn Fn() -> Option<Panel> + Send + Sync>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Panel {
    pub title: String,
    pub rows: Vec<Vec<String>>,
    pub accent: Vec<usize>,
    pub refresh: bool,
}

impl Panel {
    pub(crate) fn widths(&self) -> Vec<usize> {
        let mut widths = Vec::new();
        for row in &self.rows {
            widths.resize(widths.len().max(row.len()), 0);
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.chars().count());
            }
        }
        widths
    }

    pub(crate) fn aligned(&self) -> Vec<Vec<String>> {
        let widths = self.widths();
        let numeric: Vec<bool> = (0..widths.len())
            .map(|column| {
                self.rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .filter(|cell| !cell.is_empty())
                    .all(|cell| cell.starts_with(|first: char| first.is_ascii_digit()))
            })
            .collect();
        self.rows
            .iter()
            .map(|row| {
                row.iter()
                    .zip(widths.iter().zip(&numeric))
                    .map(|(cell, (width, numeric))| match numeric {
                        true => format!("{cell:>width$}"),
                        false => format!("{cell:<width$}"),
                    })
                    .collect()
            })
            .collect()
    }
}

impl ModelClients {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn openai(
        mut self,
        openai: impl Fn() -> nanocode::codex::OpenAiBuilder + Send + Sync + 'static,
    ) -> Self {
        self.openai = Some(std::sync::Arc::new(openai));
        self
    }

    #[must_use]
    pub fn claude(mut self, claude: impl Fn() -> ClaudeClient + Send + Sync + 'static) -> Self {
        self.claude = Some(std::sync::Arc::new(claude));
        self
    }

    #[must_use]
    pub fn openai_endpoint(
        mut self,
        base_url: impl Into<String>,
        layer: impl nanocode::http::Layer + Clone,
    ) -> Self {
        self.openai = Some(gateways::openai_endpoint(base_url, layer));
        self
    }

    #[must_use]
    pub fn claude_endpoint(
        mut self,
        messages_url: impl Into<String>,
        layer: impl nanocode::http::Layer + Clone,
    ) -> Self {
        self.claude = Some(gateways::claude_endpoint(messages_url, layer));
        self
    }

    #[must_use]
    pub fn warm_up<F>(mut self, warm_up: impl Fn() -> F + Send + Sync + 'static) -> Self
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.warm_up = Some(std::sync::Arc::new(move || Box::pin(warm_up()) as WarmUp));
        self
    }

    #[must_use]
    pub fn status(mut self, status: impl Fn() -> Option<String> + Send + Sync + 'static) -> Self {
        self.status = Some(std::sync::Arc::new(status));
        self
    }

    #[must_use]
    pub fn notices<S>(mut self, notices: impl Fn() -> S + Send + Sync + 'static) -> Self
    where
        S: futures_util::Stream<Item = String> + Send + 'static,
    {
        self.notices = Some(std::sync::Arc::new(move || Box::pin(notices()) as Notices));
        self
    }

    #[must_use]
    pub fn panel(
        mut self,
        command: &'static str,
        about: &'static str,
        panel: impl Fn() -> Option<Panel> + Send + Sync + 'static,
    ) -> Self {
        self.panel = Some(PanelSource {
            command,
            about,
            panel: std::sync::Arc::new(panel),
        });
        self
    }

    #[must_use]
    pub fn tool_environment(
        mut self,
        variables: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.tool_environment.extend(
            variables
                .into_iter()
                .map(|(name, value)| (name.into(), value.into())),
        );
        self
    }

    #[must_use]
    pub fn mcp_server(
        mut self,
        name: impl Into<String>,
        server: nanocode::tools::mcp::McpServer,
    ) -> Self {
        self.mcp_servers.push((name.into(), server));
        self
    }

    #[must_use]
    pub fn models(mut self, models: impl IntoIterator<Item = Model>) -> Self {
        self.models = Some(models.into_iter().collect());
        self
    }

    #[must_use]
    pub(crate) fn gateway(mut self, gateway: Gateway) -> Self {
        self.gateways.push(gateway);
        self
    }

    #[must_use]
    pub(crate) fn prefer(mut self, name: impl Into<String>) -> Self {
        self.preferred = Some(name.into());
        self
    }

    pub(crate) fn preferred(&self) -> Option<&str> {
        self.preferred.as_deref()
    }

    pub(crate) fn catalog(&self) -> Vec<Gateway> {
        gateways::native(
            self.openai.clone(),
            self.claude.clone(),
            self.models.as_deref(),
        )
        .into_iter()
        .chain(self.gateways.iter().cloned())
        .collect()
    }

    pub fn parse_model(&self, value: &str) -> Result<ModelSelection> {
        let gateways = self.catalog();
        gateways::parse(&gateways, value)
            .filter(|model| gateways.iter().any(|gateway| gateway.serves(model)))
            .ok_or_else(|| gateways::unknown(&gateways, value))
    }

    pub(crate) fn panel_command(&self) -> Option<(&'static str, &'static str)> {
        self.panel
            .as_ref()
            .map(|source| (source.command, source.about))
    }

    pub(crate) fn render_panel(&self) -> Option<Panel> {
        self.panel.as_ref().and_then(|source| (source.panel)())
    }

    pub(crate) fn notifying(&self) -> Option<Notices> {
        self.notices.as_ref().map(|notices| notices())
    }

    pub(crate) fn describe(&self) -> Option<String> {
        self.status.as_ref()?()
    }

    pub(crate) fn warming(&self) -> Option<WarmUp> {
        self.warm_up.as_ref().map(|warm_up| warm_up())
    }

    pub(crate) const fn supplies_openai(&self) -> bool {
        self.openai.is_some()
    }

    pub(crate) const fn reports_spend(&self) -> bool {
        self.panel.is_some() || self.status.is_some() || self.notices.is_some()
    }
}

#[derive(Clone)]
pub struct HostConfig {
    pub workspace: PathBuf,
    pub store: Option<SessionStore>,
    pub subagents: bool,
    pub runners: Vec<Runner>,
    pub runner_servers: Vec<RunnerServerConfig>,
    pub tab: Option<String>,
    pub routes: Routes,
    pub clients: ModelClients,
    pub headless: bool,
}

#[derive(Clone)]
pub struct Host {
    pub config: HostConfig,
    pub runtime: Nanocode,
    hand: std::sync::Arc<nanocode_sync::Atom<Option<nanocode::HandDescription>>>,
}

pub fn listener_notice(server: &nanocode::RunnerServer) -> String {
    format!(
        "listening {} · token {}\nStart a hand: nc hand {} --token {}",
        server.endpoint(),
        server.token(),
        server.endpoint(),
        server.token()
    )
}

fn claude_environment(hand: &nanocode::HandDescription) -> nanocode::claude::ClaudeEnvironment {
    let mut environment = nanocode::claude::ClaudeEnvironment::new();
    if let Some(directory) = &hand.workspace {
        environment = environment.working_directory(directory.as_str());
    }
    if let Some(described) = &hand.environment {
        if let Some(platform) = &described.platform {
            environment = environment.platform(platform.as_str());
        }
        if let Some(shell) = &described.shell {
            environment = environment.shell(shell.as_str());
        }
        if let Some(version) = &described.os_version {
            environment = environment.os_version(version.as_str());
        }
    }
    environment
}

fn codex_environment(hand: &nanocode::HandDescription) -> nanocode::agent::ExecutionEnvironment {
    let mut environment = nanocode::agent::ExecutionEnvironment::local();
    if let Some(directory) = &hand.workspace {
        environment = environment.working_directory(directory.as_str());
    }
    if let Some(shell) = hand
        .environment
        .as_ref()
        .and_then(|described| described.shell.as_deref())
    {
        environment = environment.shell(shell);
    }
    environment
}

impl Host {
    pub async fn build(
        config: HostConfig,
        model: &ModelSelection,
        thinking: Thinking,
    ) -> Result<Self> {
        let routes = config.routes.clone();
        let openai = routes
            .default_openai()
            .ok_or("no gateway offers Codex models")?;
        let claude = routes
            .default_claude()
            .ok_or("no gateway offers Claude models")?;
        let hand = std::sync::Arc::new(nanocode_sync::Atom::new(None::<nanocode::HandDescription>));
        let described = std::sync::Arc::clone(&hand);
        let routed = routes.clone();
        let codex = CodexHarness::new(openai)
            .client_for(move |request| routed.openai(request))
            .configure(move |builder| {
                Ok(match &*described.get() {
                    Some(hand) => builder.execution_environment(codex_environment(hand)),
                    None => builder,
                })
            });
        let described = std::sync::Arc::clone(&hand);
        let claude = ClaudeHarness::new(claude)
            .client_for(move |request| routes.claude(request))
            .headless(config.headless)
            .configure(move |builder| {
                Ok(match &*described.get() {
                    Some(hand) => builder.environment(claude_environment(hand)),
                    None => builder,
                })
            });
        let mut builder = Nanocode::builder()
            .openai(codex)
            .claude(claude)
            .workspace(config.workspace.clone())
            .tool_environment(config.clients.tool_environment.clone());
        if !config.clients.mcp_servers.is_empty() {
            let mcp = config
                .clients
                .mcp_servers
                .iter()
                .fold(
                    nanocode::tools::mcp::Mcp::builder(),
                    |mcp, (name, server)| mcp.server(name, server.clone()),
                )
                .build()?;
            builder = builder.tool_provider(mcp);
        }
        for runner in &config.runners {
            builder = builder.runner(runner.clone());
        }
        for server in &config.runner_servers {
            builder = builder.runner_server(server.clone());
        }
        builder = match &config.store {
            Some(store) => builder.durability(store.clone()),
            None => builder.durability(nanocode::durability::MemoryStore::new()?),
        };
        if !config.subagents {
            builder = builder.without_subagents();
        }
        let builder = match (model.as_codex(), model.as_claude()) {
            (Some(model), _) => {
                builder
                    .for_backend::<Codex>()
                    .model(model)
                    .thinking(thinking)
                    .build_async()
                    .await
            }
            (_, Some(model)) => {
                builder
                    .for_backend::<Claude>()
                    .model(model)
                    .thinking(thinking)
                    .build_async()
                    .await
            }
            _ => return Err(format!("unsupported model {model}").into()),
        };
        Ok(Self {
            config,
            runtime: builder?,
            hand,
        })
    }

    pub async fn replacement_hand(&self) {
        if let Some((_, description)) = self.runtime.runner_pool().replacement_hand().await {
            self.hand
                .set(description.filter(|description| !description.tools_only));
        }
    }

    pub async fn start(
        &self,
        model: &ModelSelection,
        thinking: Thinking,
    ) -> Result<(Agent, AgentEvents)> {
        self.replacement_hand().await;
        let started = match (model.as_codex(), model.as_claude()) {
            (Some(model), _) => {
                self.runtime
                    .spawn()
                    .backend::<Codex>()
                    .model(model)
                    .thinking(thinking)
                    .build()
                    .await
            }
            (_, Some(model)) => {
                self.runtime
                    .spawn()
                    .backend::<Claude>()
                    .model(model)
                    .thinking(thinking)
                    .build()
                    .await
            }
            _ => return Err(format!("unsupported model {model}").into()),
        };
        Ok(started?)
    }

    pub async fn open(
        &self,
        store: &SessionStore,
        stored: &nanocode::durability::StoredSession,
    ) -> Result<(Agent, AgentEvents)> {
        self.replacement_hand().await;
        Ok(self.runtime.open_loaded(store, stored).await?)
    }
}

pub fn parse_thinking(value: &str) -> Result<Thinking> {
    Thinking::ALL
        .into_iter()
        .find(|thinking| thinking.as_str().eq_ignore_ascii_case(value.trim()))
        .ok_or_else(|| {
            let known = Thinking::ALL.map(Thinking::as_str).join(", ");
            format!("unknown thinking level {value:?}; expected one of {known}").into()
        })
}

pub fn parse_tier(value: &str) -> Result<ServiceTier> {
    match value {
        "standard" => Ok(ServiceTier::Standard),
        "fast" => Ok(ServiceTier::Fast),
        "ultrafast" => Ok(ServiceTier::Ultrafast),
        other => Err(format!("unknown service tier {other:?}").into()),
    }
}

pub fn home_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(home) = explicit {
        return Ok(home.to_path_buf());
    }
    let home = nanocode_home::AgentHome::from_env()?;
    Ok(home.nanocode_home().to_path_buf())
}

pub async fn open_store(home: &Path) -> Result<SessionStore> {
    let store = nanocode_store_jsonl::JsonlStore::open_home_in_background(home)?;
    Ok(nanocode::session_store(store)?)
}

const HAND_RELAY: &str = "wss://nanocode.xyz/api/hands";
const HAND_PAGE: &str = "https://nanocode.xyz/hand";

pub fn tab_notice(link: &str) -> String {
    format!(
        "Make a browser tab this session's hand: open {link} and click Attach. The session starts once the tab attaches."
    )
}

pub struct RunnerConfigs {
    pub runners: Vec<Runner>,
    pub servers: Vec<RunnerServerConfig>,
    pub tab: Option<String>,
}

pub fn runner_configs(args: &RemoteArgs) -> Result<RunnerConfigs> {
    let mut runners = Vec::new();
    let mut servers = Vec::new();
    if args.tab {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)?;
        let token = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut runner = Runner::connect(format!("{HAND_RELAY}?role=brain&wait"))
            .token(&token)
            .replace();
        if let Some(name) = &args.runner_name {
            runner = runner.name(name);
        }
        if let Some(ms) = args.runner_call_timeout_ms {
            runner = runner.call_timeout(std::time::Duration::from_millis(ms));
        }
        runners.push(runner);
        return Ok(RunnerConfigs {
            runners,
            servers,
            tab: Some(format!("{HAND_PAGE}#url={HAND_RELAY}&token={token}")),
        });
    }
    match (&args.runner_connect, args.runner_host) {
        (Some(url), _) => {
            let token = args
                .runner_token
                .as_ref()
                .ok_or("--runner-connect requires --runner-token or NANOCODE_RUNNER_TOKEN")?;
            let mut runner = Runner::connect(url).token(token);
            if let Some(name) = &args.runner_name {
                runner = runner.name(name);
            }
            runner = match args.runner_mode {
                RunnerModeArg::Alongside => runner.alongside(),
                RunnerModeArg::Replace => runner.replace(),
            };
            if let Some(workspace) = &args.runner_workspace {
                runner = runner.workspace(workspace.clone());
            }
            if let Some(ms) = args.runner_call_timeout_ms {
                runner = runner.call_timeout(std::time::Duration::from_millis(ms));
            }
            runners.push(runner);
        }
        (None, Some(address)) => {
            let mut server = RunnerServerConfig::listen(address.to_string());
            if let Some(token) = &args.runner_token {
                server = server.token(token);
            }
            server = match args.runner_mode {
                RunnerModeArg::Alongside => server.alongside(),
                RunnerModeArg::Replace => server.replace(),
            };
            if let Some(workspace) = &args.runner_workspace {
                server = server.workspace(workspace.clone());
            }
            if let Some(ms) = args.runner_call_timeout_ms {
                server = server.call_timeout(std::time::Duration::from_millis(ms));
            }
            servers.push(server);
        }
        (None, None) if matches!(args.runner_mode, RunnerModeArg::Replace) => {
            return Err("replace mode requires --runner-connect or --runner-host".into());
        }
        (None, None) => {}
    }
    Ok(RunnerConfigs {
        runners,
        servers,
        tab: None,
    })
}
