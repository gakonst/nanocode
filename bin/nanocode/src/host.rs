use std::path::{Path, PathBuf};

use nanocode::{
    Claude, ClaudeHarness, ClaudeModel, Codex, CodexHarness, Model, ModelSelection, Nanocode,
    OpenAi, Runner, RunnerServerConfig, ServiceTier, Thinking,
    agent::{Agent, AgentEvents},
    claude::ClaudeClient,
    durability::SessionStore,
};

use crate::cli::{GatewayArg, GatewayArgs, RemoteArgs, RunnerModeArg};

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
        self,
        base_url: impl Into<String>,
        layer: impl nanocode::http::Layer + Clone,
    ) -> Self {
        let base_url = base_url.into();
        self.openai(move || {
            OpenAi::builder(nanocode::codex::OpenAiAuth::gateway_managed())
                .api_base_url(base_url.clone())
                .transport(nanocode::codex::transport::ResponsesTransport::Https)
                .http_layer(layer.clone())
        })
    }

    #[must_use]
    pub fn claude_endpoint(
        self,
        messages_url: impl Into<String>,
        layer: impl nanocode::http::Layer + Clone,
    ) -> Self {
        let messages_url = messages_url.into();
        self.claude(move || {
            ClaudeClient::with_auth_headers(
                Default::default(),
                messages_url.clone(),
                Default::default(),
            )
            .with_http_layer(layer.clone())
        })
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
}

#[derive(Clone)]
pub struct HostConfig {
    pub workspace: PathBuf,
    pub store: Option<SessionStore>,
    pub subagents: bool,
    pub runners: Vec<Runner>,
    pub runner_servers: Vec<RunnerServerConfig>,
    pub gateway: Option<Gateway>,
    pub clients: ModelClients,
    pub headless: bool,
}

#[derive(Clone)]
pub struct Gateway {
    route: nanocode::codex::transport::ResponsesGateway,
    auth: nanocode::codex::OpenAiAuth,
    credential: &'static str,
    base_url: String,
}

impl Gateway {
    pub fn serves(&self, model: &ModelSelection) -> bool {
        model
            .as_codex()
            .is_none_or(|model| self.route.serves(model))
    }

    pub fn check(&self, model: &ModelSelection) -> Result<()> {
        if self.serves(model) {
            return Ok(());
        }
        let served = ModelSelection::for_family(nanocode::HarnessFamily::Codex)
            .filter(|candidate| self.serves(candidate))
            .map(|candidate| candidate.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "{model} is not available through the {} gateway; choose one of {served} or a Claude model",
            self.route.name()
        )
        .into())
    }

    pub const fn name(&self) -> &'static str {
        self.route.name()
    }

    pub fn compacts(&self, model: &ModelSelection) -> bool {
        model.as_codex().is_none() || self.route.supports_inline_compaction()
    }

    pub fn describe(&self) -> String {
        format!(
            "Codex via {} {} · credential {}",
            self.route.name(),
            self.base_url,
            self.credential
        )
    }
}

fn env_secret(name: &'static str) -> Option<(&'static str, String)> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| (name, value))
}

pub fn gateway(args: &GatewayArgs) -> Result<Option<Gateway>> {
    use nanocode::codex::{OpenAiAuth, transport::ResponsesGateway};
    let Some(kind) = args.gateway else {
        return Ok(None);
    };
    let account = || {
        args.cloudflare_account
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "--gateway {} needs --cloudflare-account or CLOUDFLARE_ACCOUNT_ID",
                    kind_name(kind)
                )
            })
    };
    let missing = |names: &str| -> Error {
        format!(
            "--gateway {} needs {names} in the environment",
            kind_name(kind)
        )
        .into()
    };
    let (mut route, auth, credential) = match kind {
        GatewayArg::Openrouter => {
            let (name, key) =
                env_secret("OPENROUTER_API_KEY").ok_or_else(|| missing("OPENROUTER_API_KEY"))?;
            (
                ResponsesGateway::openrouter(),
                OpenAiAuth::api_key(key),
                name,
            )
        }
        GatewayArg::Vercel => {
            let (name, key) = env_secret("AI_GATEWAY_API_KEY")
                .or_else(|| env_secret("VERCEL_OIDC_TOKEN"))
                .ok_or_else(|| missing("AI_GATEWAY_API_KEY or VERCEL_OIDC_TOKEN"))?;
            (ResponsesGateway::vercel(), OpenAiAuth::api_key(key), name)
        }
        GatewayArg::Cloudflare => {
            let (name, key) = env_secret("CLOUDFLARE_API_TOKEN")
                .ok_or_else(|| missing("CLOUDFLARE_API_TOKEN"))?;
            let mut route = ResponsesGateway::cloudflare(account()?);
            if let Some(gateway) = args
                .cloudflare_gateway
                .clone()
                .filter(|value| !value.trim().is_empty())
            {
                route = route.gateway_id(gateway);
            }
            (route, OpenAiAuth::api_key(key), name)
        }
        GatewayArg::CloudflareGateway => {
            let gateway = args
                .cloudflare_gateway
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or("--gateway cloudflare-gateway needs --cloudflare-gateway or CLOUDFLARE_GATEWAY_ID")?;
            let mut route = ResponsesGateway::cloudflare_gateway(account()?, gateway);
            let gateway_token = env_secret("CF_AIG_TOKEN");
            if let Some((_, token)) = &gateway_token {
                route = route.gateway_token(token.clone());
            }
            match (
                env_secret("OPENAI_API_KEY"),
                gateway_token,
                env_secret("CLOUDFLARE_API_TOKEN"),
            ) {
                (Some((name, key)), _, _) => (route, OpenAiAuth::api_key(key), name),
                (None, Some(_), _) => (route, OpenAiAuth::gateway_managed(), "CF_AIG_TOKEN"),
                (None, None, Some((name, key))) => (route, OpenAiAuth::api_key(key), name),
                (None, None, None) => {
                    return Err(missing(
                        "OPENAI_API_KEY, CF_AIG_TOKEN or CLOUDFLARE_API_TOKEN",
                    ));
                }
            }
        }
    };
    if let Some(url) = &args.gateway_base_url {
        route = route.base_url(url.clone());
    }
    let base_url = route.api_base_url().to_owned();
    Ok(Some(Gateway {
        route,
        auth,
        credential,
        base_url,
    }))
}

const fn kind_name(kind: GatewayArg) -> &'static str {
    match kind {
        GatewayArg::Openrouter => "openrouter",
        GatewayArg::Vercel => "vercel",
        GatewayArg::Cloudflare => "cloudflare",
        GatewayArg::CloudflareGateway => "cloudflare-gateway",
    }
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
        let openai = match (&config.clients.openai, &config.gateway) {
            (Some(openai), _) => openai(),
            (None, Some(gateway)) => {
                gateway.check(model)?;
                OpenAi::builder(gateway.auth.clone()).responses_gateway(gateway.route.clone())
            }
            (None, None) => OpenAi::builder(nanocode::codex::OpenAiAuth::chatgpt()),
        }
        .raw_api_events(false)
        .build()?;
        let hand = std::sync::Arc::new(nanocode_sync::Atom::new(None::<nanocode::HandDescription>));
        let described = std::sync::Arc::clone(&hand);
        let codex = CodexHarness::new(openai).configure(move |builder| {
            Ok(match &*described.get() {
                Some(hand) => builder.execution_environment(codex_environment(hand)),
                None => builder,
            })
        });
        let described = std::sync::Arc::clone(&hand);
        let claude = ClaudeHarness::new(
            config
                .clients
                .claude
                .as_ref()
                .map_or_else(ClaudeClient::default, |claude| claude()),
        )
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

pub fn parse_model(value: &str) -> Result<ModelSelection> {
    let value = value.trim();
    if let Some(model) = Model::known(value) {
        return Ok(ModelSelection::Codex(model));
    }
    if let Ok(model) = value.parse::<ClaudeModel>() {
        return Ok(ModelSelection::Claude(model));
    }
    let known = ModelSelection::for_family(nanocode::HarnessFamily::Codex)
        .chain(ModelSelection::for_family(nanocode::HarnessFamily::Claude))
        .map(|model| model.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!("unknown model {value:?}; known models: {known}").into())
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
    let store = nanocode_store_jsonl::JsonlStore::open_home_async(home).await?;
    Ok(nanocode::session_store(store)?)
}

pub fn runner_configs(args: &RemoteArgs) -> Result<(Vec<Runner>, Vec<RunnerServerConfig>)> {
    let mut runners = Vec::new();
    let mut servers = Vec::new();
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
    Ok((runners, servers))
}
