use std::path::{Path, PathBuf};

use nanocode::{
    ClaudeHarness, ClaudeModel, CodexHarness, Model, ModelSelection, Nanocode, OpenAi, Runner,
    RunnerServerConfig, ServiceTier, Thinking,
    agent::{Agent, AgentEvents},
    claude::ClaudeClient,
    durability::SessionStore,
};

use crate::{
    cli::{GatewayArg, GatewayArgs, RemoteArgs, RunnerModeArg},
    tempo::Tempo,
};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

type WarmUp = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
pub(crate) type Answer = std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
pub(crate) type Notices = std::pin::Pin<Box<dyn nanocode_sync::Stream<Item = String> + Send>>;

#[derive(Clone, Default)]
pub struct ModelClients {
    openai: Option<std::sync::Arc<dyn Fn() -> nanocode::codex::OpenAiBuilder + Send + Sync>>,
    claude: Option<std::sync::Arc<dyn Fn() -> ClaudeClient + Send + Sync>>,
    warm_up: Option<std::sync::Arc<dyn Fn() -> WarmUp + Send + Sync>>,
    status: Option<std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    notices: Option<std::sync::Arc<dyn Fn() -> Notices + Send + Sync>>,
    answer: Option<Answer>,
    panel: Option<PanelSource>,
    tool_environment: Vec<(String, String)>,
    mcp_servers: Vec<(String, nanocode::tools::mcp::McpServer)>,
    models: Option<std::sync::Arc<[Model]>>,
    unlisted: Option<nanocode::HarnessFamily>,
    context_windows: std::sync::Arc<[(Model, u64)]>,
    tempo: Option<Tempo>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Route {
    #[default]
    Native,
    Tempo,
    Boundless,
}

impl Route {
    pub const fn provider(self) -> Option<&'static str> {
        match self {
            Self::Native => None,
            Self::Tempo => Some(Tempo::NAME),
            Self::Boundless => Some(Tempo::BOUNDLESS),
        }
    }

    pub fn of(provider: Option<&str>) -> Result<Self> {
        match provider {
            None => Ok(Self::Native),
            Some(Tempo::NAME) => Ok(Self::Tempo),
            Some(Tempo::BOUNDLESS) => Ok(Self::Boundless),
            Some(other) => Err(format!("this build does not route models through {other}").into()),
        }
    }

    pub fn label(self, model: &ModelSelection) -> String {
        match self {
            Self::Native => model.to_string(),
            Self::Tempo => format!("{}{model}", Tempo::PREFIX),
            Self::Boundless => format!("{}{}/{model}", Tempo::PREFIX, Tempo::BOUNDLESS),
        }
    }
}

pub fn follows_harness(route: Route, model: &ModelSelection) -> bool {
    route != Route::Boundless
        && model.as_claude().is_none()
        && model
            .as_codex()
            .is_none_or(|model| !Model::ALL.contains(&model))
}

fn native_model(model: &ModelSelection) -> bool {
    model.as_claude().is_some()
        || model
            .as_codex()
            .is_some_and(|model| Model::ALL.contains(&model))
}

pub fn on_harness(model: &ModelSelection, family: &nanocode::HarnessFamily) -> ModelSelection {
    let moved = ModelSelection::new(family.clone(), model.as_str());
    match (moved.as_codex(), moved.as_claude()) {
        (Some(model), _) => ModelSelection::Codex(model),
        (_, Some(model)) => ModelSelection::Claude(model),
        _ => moved,
    }
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
        S: nanocode_sync::Stream<Item = String> + Send + 'static,
    {
        self.notices = Some(std::sync::Arc::new(move || Box::pin(notices()) as Notices));
        self
    }

    #[must_use]
    pub fn answer(
        mut self,
        answer: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.answer = Some(std::sync::Arc::new(answer));
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
    pub(crate) fn unlisted_models(mut self, family: nanocode::HarnessFamily) -> Self {
        self.unlisted = Some(family);
        self
    }

    #[must_use]
    pub(crate) fn context_windows(
        mut self,
        windows: impl IntoIterator<Item = (Model, u64)>,
    ) -> Self {
        self.context_windows = windows.into_iter().collect();
        self
    }

    pub(crate) fn codex_models(&self) -> Option<&[Model]> {
        self.models.as_deref()
    }

    pub(crate) fn default_model(&self, family: &nanocode::HarnessFamily) -> ModelSelection {
        match self.models.as_deref().and_then(<[_]>::first) {
            Some(model) if *family == nanocode::HarnessFamily::Codex => {
                ModelSelection::Codex(*model)
            }
            _ => family.default_model(),
        }
    }

    #[must_use]
    pub(crate) fn tempo(mut self, tempo: Tempo) -> Self {
        self.tempo = Some(tempo);
        self
    }

    pub(crate) const fn tempo_models(&self) -> Option<&Tempo> {
        self.tempo.as_ref()
    }

    pub(crate) fn resolve(&self, value: &str) -> Result<(Route, ModelSelection)> {
        let value = value.trim();
        let Some(id) = value.strip_prefix(Tempo::PREFIX) else {
            return Ok((Route::Native, self.parse_model(value)?));
        };
        if self.tempo.is_none() {
            return Err(format!(
                "{value} runs on the Tempo gateway, which this build does not offer"
            )
            .into());
        }
        let id = id.trim();
        if id.is_empty() {
            return Err(format!(
                "name a model after {}, such as {}claude-opus-5-5",
                Tempo::PREFIX,
                Tempo::PREFIX
            )
            .into());
        }
        if let Some(id) = id
            .strip_prefix(Tempo::BOUNDLESS)
            .and_then(|id| id.strip_prefix('/'))
        {
            if id.trim().is_empty() {
                return Err(format!(
                    "name a model after {}{}/, such as {}{}/z-ai/glm-5.3-flash",
                    Tempo::PREFIX,
                    Tempo::BOUNDLESS,
                    Tempo::PREFIX,
                    Tempo::BOUNDLESS
                )
                .into());
            }
            return Ok((Route::Boundless, Tempo::boundless_model(id.trim())));
        }
        Ok((Route::Tempo, Tempo::model(id)))
    }

    pub fn parse_model(&self, value: &str) -> Result<ModelSelection> {
        let listed = self.listed_model(value);
        match &self.unlisted {
            Some(family) if listed.is_err() && !value.trim().is_empty() => {
                Ok(ModelSelection::new(family.clone(), value.trim()))
            }
            _ => listed,
        }
    }

    fn listed_model(&self, value: &str) -> Result<ModelSelection> {
        let Some(served) = &self.models else {
            return parse_model(value);
        };
        let value = value.trim();
        if let Some(model) = served
            .iter()
            .find(|model| model.as_str() == value || Model::known(value) == Some(**model))
        {
            return Ok(ModelSelection::Codex(*model));
        }
        if let Ok(model) = value.parse::<ClaudeModel>() {
            return Ok(ModelSelection::Claude(model));
        }
        Err(unknown_model(value, self))
    }

    pub(crate) fn panel_command(&self) -> Option<(&'static str, &'static str)> {
        self.panel
            .as_ref()
            .map(|source| (source.command, source.about))
    }

    pub(crate) fn render_panel(&self) -> Option<Panel> {
        self.panel.as_ref().and_then(|source| (source.panel)())
    }

    pub(crate) fn answering(&self) -> Option<Answer> {
        self.answer.clone()
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

    pub(crate) const fn supplies_claude(&self) -> bool {
        self.claude.is_some()
    }

    pub(crate) const fn reports_spend(&self) -> bool {
        self.panel.is_some() || self.status.is_some() || self.notices.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subagents {
    Off,
    NativeModels,
    AllModels,
}

#[derive(Clone)]
pub struct HostConfig {
    pub workspace: PathBuf,
    pub store: Option<SessionStore>,
    pub archiver: Option<nanocode_store_jsonl::JsonlArchiver>,
    pub subagents: Subagents,
    pub runners: Vec<Runner>,
    pub runner_servers: Vec<RunnerServerConfig>,
    pub tab: Option<String>,
    pub gateway: Option<Gateway>,
    pub clients: ModelClients,
    pub headless: bool,
    pub cyber_access_program: Option<nanocode::codex::CyberAccessProgram>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u32>,
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
        .cyber_access_program(config.cyber_access_program)
        .build()?;
        let install = install_id();
        let identified = |client: ClaudeClient| {
            if client.uses_subscription() {
                client
            } else {
                client.with_install_id(install.clone())
            }
        };
        let (tempo_openai, tempo_claude, boundless) = match &config.clients.tempo {
            Some(tempo) => (
                Some(tempo.openai().raw_api_events(false).build()?),
                Some(identified(tempo.claude())),
                Some(tempo.boundless().raw_api_events(false).build()?),
            ),
            None => (None, None, None),
        };
        let route = |request: &nanocode::HarnessRequest| {
            Route::of(request.options.selected_provider()).ok()
        };
        let context_window = config.context_window;
        let listed = std::sync::Arc::clone(&config.clients.context_windows);
        let max_output = config.max_output_tokens;
        if let (Some(tokens), Some(maximum)) = (
            context_window,
            model.as_codex().and_then(|model| model.context_window_limit()),
        ) && tokens > maximum
        {
            return Err(format!(
                "--context-window {tokens} is over {}'s window of {maximum} tokens",
                model.as_str()
            )
            .into());
        }
        let hand = std::sync::Arc::new(nanocode_sync::Atom::new(None::<nanocode::HandDescription>));
        let described = std::sync::Arc::clone(&hand);
        let codex = CodexHarness::new(openai)
            .client_for(move |request| match route(request) {
                Some(Route::Tempo) => tempo_openai.clone(),
                Some(Route::Boundless) => boundless.clone(),
                _ => None,
            })
            .direct_tools_for(|request| !native_model(&request.model))
            .direct_subagents(config.subagents == Subagents::AllModels)
            .configure(move |builder| {
                let window = context_window
                    .or_else(|| listed_window(&listed, builder.selected_model().as_str()));
                let builder = match window {
                    Some(tokens) => builder.context_window_tokens(tokens),
                    None => builder,
                };
                Ok(match &*described.get() {
                    Some(hand) => builder.execution_environment(codex_environment(hand)),
                    None => builder,
                })
            });
        let described = std::sync::Arc::clone(&hand);
        let listed = std::sync::Arc::clone(&config.clients.context_windows);
        let claude = ClaudeHarness::new(identified(
            config
                .clients
                .claude
                .as_ref()
                .map_or_else(ClaudeClient::default, |claude| claude()),
        ))
        .client_for(move |request| {
            tempo_claude
                .clone()
                .filter(|_| route(request) == Some(Route::Tempo))
        })
        .direct_tools_for(|request| !native_model(&request.model))
        .direct_subagents(config.subagents == Subagents::AllModels)
        .headless(config.headless)
        .configure(move |builder| {
            let builder = match max_output {
                Some(tokens) => builder.max_tokens(tokens),
                None if builder.knows_max_tokens() => builder,
                None => builder.max_tokens(Tempo::OUTPUT_TOKENS),
            };
            let window =
                context_window.or_else(|| listed_window(&listed, builder.selected_model()));
            let builder = match window {
                Some(tokens) => builder.context_window_tokens(tokens),
                None => builder,
            };
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
        if config.subagents == Subagents::Off {
            builder = builder.without_subagents();
        }
        let builder = builder.harness_model(model.clone()).thinking(thinking);
        let runtime = if config.headless {
            builder.build_async().await?
        } else {
            builder.build()?
        };
        if let (Some(subagents), Some(tempo)) = (runtime.subagents(), &config.clients.tempo) {
            tempo.offer(subagents, !config.headless);
        }
        Ok(Self {
            config,
            runtime,
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
        route: Route,
    ) -> Result<(Agent, AgentEvents)> {
        self.replacement_hand().await;
        let spawn = match route.provider() {
            Some(provider) => self.runtime.spawn().provider(provider),
            None => self.runtime.spawn(),
        };
        Ok(spawn
            .harness_model(model.clone())
            .thinking(thinking)
            .build()
            .await?)
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

fn listed_window(windows: &[(Model, u64)], model: &str) -> Option<u64> {
    windows
        .iter()
        .find(|(listed, _)| listed.as_str() == model)
        .map(|(_, tokens)| *tokens)
}

pub fn parse_model(value: &str) -> Result<ModelSelection> {
    let value = value.trim();
    if let Some(model) = Model::known(value) {
        return Ok(ModelSelection::Codex(model));
    }
    if let Ok(model) = value.parse::<ClaudeModel>() {
        return Ok(ModelSelection::Claude(model));
    }
    Err(unknown_model(value, &ModelClients::default()))
}

fn unknown_model(value: &str, clients: &ModelClients) -> Error {
    let known = crate::models::catalog(clients)
        .into_iter()
        .flat_map(|group| group.models)
        .filter(|(route, _, _)| *route == Route::Native)
        .map(|(_, model, _)| model.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("unknown model {value:?}; known models: {known}; models the Tempo gateway serves are named {}<model> ('nanocode models' lists them)", Tempo::PREFIX).into()
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

fn install_id() -> String {
    let path = home_dir(None).map(|home| home.join("install_id"));
    if let Ok(path) = &path
        && let Ok(id) = std::fs::read_to_string(path)
        && nanocode_platform::Uuid::parse_str(&id).is_ok()
    {
        return id;
    }
    let id = nanocode_platform::uuid_v4().to_string();
    if let Ok(path) = &path {
        let _ = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, &id));
    }
    id
}

pub fn home_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(home) = explicit {
        return Ok(home.to_path_buf());
    }
    let home = nanocode_home::AgentHome::from_env()?;
    Ok(home.nanocode_home().to_path_buf())
}

pub async fn open_store(
    home: &Path,
) -> Result<(SessionStore, nanocode_store_jsonl::JsonlArchiver)> {
    let store = nanocode_store_jsonl::JsonlStore::open_home_in_background(home)?;
    let archiver = store.archiver();
    Ok((nanocode::session_store(store)?, archiver))
}

pub async fn archive(config: &HostConfig) -> Result<()> {
    if let (Some(store), Some(archiver)) = (&config.store, &config.archiver) {
        store.summarize(&archiver.written().await?).await?;
        archiver.archive().await?;
    }
    Ok(())
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
        nanocode_platform::fill_random(&mut bytes)?;
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
