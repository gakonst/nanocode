use std::{collections::HashMap, sync::Arc};

use nanocode::{
    ClaudeClient, ClaudeModel, HarnessFamily, HarnessRequest, Model, ModelSelection, OpenAi,
    codex::{
        OpenAiAuth, OpenAiBuilder,
        transport::{ResponsesGateway, ResponsesTransport},
    },
};
use nanocode_sync::{Atom, SingleFlight};
use serde_json::Value;

use crate::{
    cli::{GatewayArg, GatewayArgs},
    host::{ModelClients, Result},
};

pub(crate) type OpenAiSource = Arc<dyn Fn() -> OpenAiBuilder + Send + Sync>;
pub(crate) type ClaudeSource = Arc<dyn Fn() -> ClaudeClient + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Gateway {
    pub(crate) name: String,
    pub(crate) about: String,
    pub(crate) login: bool,
    codex: Option<(OpenAiSource, Served)>,
    claude: Option<ClaudeSource>,
}

#[derive(Clone)]
enum Served {
    Known,
    Routed(ResponsesGateway),
    Listed(Arc<Listing>),
}

pub(crate) struct Listing {
    url: String,
    state: Atom<Listed>,
    flight: SingleFlight<()>,
}

pub(crate) enum Listed {
    Loading,
    Models(Vec<Model>),
    Failed(String),
}

impl Listing {
    pub(crate) fn fixed(models: impl IntoIterator<Item = Model>) -> Arc<Self> {
        Arc::new(Self {
            url: String::new(),
            state: Atom::new(Listed::Models(models.into_iter().collect())),
            flight: SingleFlight::default(),
        })
    }

    pub(crate) fn remote(url: String) -> Arc<Self> {
        Arc::new(Self {
            url,
            state: Atom::new(Listed::Loading),
            flight: SingleFlight::default(),
        })
    }

    pub(crate) async fn load(&self) {
        if self.state.read(|state| matches!(state, Listed::Models(_))) {
            return;
        }
        self.flight
            .run(|| async {
                self.state
                    .set(match list(&reqwest::Client::new(), &self.url).await {
                        Ok(models) => Listed::Models(models),
                        Err(error) => Listed::Failed(error),
                    });
            })
            .await;
    }

    fn contains(&self, model: Model) -> bool {
        self.state
            .read(|state| matches!(state, Listed::Models(models) if models.contains(&model)))
    }

    fn find(&self, id: &str) -> Option<Model> {
        self.state.read(|state| match state {
            Listed::Models(models) => models.iter().copied().find(|model| model.as_str() == id),
            _ => None,
        })
    }
}

pub(crate) async fn list(
    client: &reqwest::Client,
    url: &str,
) -> std::result::Result<Vec<Model>, String> {
    let body = async {
        client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await
    }
    .await
    .map_err(|error| format!("listing models at {url}: {error}"))?;
    let listing: Value = serde_json::from_slice(&body)
        .map_err(|error| format!("{url} did not return a model list: {error}"))?;
    let mut models = Vec::new();
    for entry in listing["data"]
        .as_array()
        .ok_or_else(|| format!("{url} returned no data list"))?
    {
        let model = entry["id"]
            .as_str()
            .ok_or_else(|| format!("{url} listed a model without an id"))?
            .parse::<Model>()?;
        if !models.contains(&model) {
            models.push(model);
        }
    }
    if models.is_empty() {
        return Err(format!("{url} lists no models"));
    }
    Ok(models)
}

impl Gateway {
    pub(crate) fn new(name: impl Into<String>, about: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            about: about.into(),
            login: false,
            codex: None,
            claude: None,
        }
    }

    const fn login(mut self) -> Self {
        self.login = true;
        self
    }

    fn codex(mut self, openai: OpenAiSource, served: Served) -> Self {
        self.codex = Some((openai, served));
        self
    }

    pub(crate) fn known(self, openai: OpenAiSource) -> Self {
        self.codex(openai, Served::Known)
    }

    pub(crate) fn listed(self, openai: OpenAiSource, listing: Arc<Listing>) -> Self {
        self.codex(openai, Served::Listed(listing))
    }

    pub(crate) fn claude(mut self, claude: ClaudeSource) -> Self {
        self.claude = Some(claude);
        self
    }

    pub(crate) fn title(&self) -> String {
        format!("{} · {}", self.name, self.about)
    }

    pub(crate) fn offers(&self, family: &HarnessFamily) -> bool {
        if *family == HarnessFamily::Claude {
            self.claude.is_some()
        } else {
            self.codex.is_some()
        }
    }

    pub(crate) fn serves(&self, model: &ModelSelection) -> bool {
        match (model.as_codex(), model.as_claude()) {
            (Some(model), _) => self.codex.as_ref().is_some_and(|(_, served)| match served {
                Served::Known => !model.is_custom(),
                Served::Routed(route) => route.serves(model),
                Served::Listed(listing) => listing.contains(model),
            }),
            (_, Some(_)) => self.claude.is_some(),
            _ => false,
        }
    }

    pub(crate) fn compacts(&self, model: &ModelSelection) -> bool {
        model.as_codex().is_none()
            || !matches!(&self.codex, Some((_, Served::Routed(route))) if !route.supports_inline_compaction())
    }

    pub(crate) fn models(&self) -> (Vec<ModelSelection>, Option<String>) {
        let mut models = Vec::new();
        let mut note = None;
        match &self.codex {
            Some((_, Served::Known)) => models.extend(Model::ALL.map(ModelSelection::Codex)),
            Some((_, Served::Routed(route))) => models.extend(
                Model::ALL
                    .into_iter()
                    .filter(|model| route.serves(*model))
                    .map(ModelSelection::Codex),
            ),
            Some((_, Served::Listed(listing))) => listing.state.read(|state| match state {
                Listed::Loading => note = Some("listing models…".to_owned()),
                Listed::Models(listed) => {
                    models.extend(listed.iter().copied().map(ModelSelection::Codex));
                }
                Listed::Failed(error) => note = Some(error.clone()),
            }),
            None => {}
        }
        if self.claude.is_some() {
            models.extend(ClaudeModel::ALL.map(ModelSelection::Claude));
        }
        (models, note)
    }

    pub(crate) fn listing(&self) -> Option<&Arc<Listing>> {
        match &self.codex {
            Some((_, Served::Listed(listing))) => Some(listing),
            _ => None,
        }
    }
}

pub(crate) fn openai_endpoint(
    base_url: impl Into<String>,
    layer: impl nanocode::http::Layer + Clone,
) -> OpenAiSource {
    let base_url = base_url.into();
    Arc::new(move || {
        OpenAi::builder(OpenAiAuth::gateway_managed())
            .api_base_url(base_url.clone())
            .transport(ResponsesTransport::Https)
            .http_layer(layer.clone())
    })
}

pub(crate) fn claude_endpoint(
    messages_url: impl Into<String>,
    layer: impl nanocode::http::Layer + Clone,
) -> ClaudeSource {
    let messages_url = messages_url.into();
    Arc::new(move || {
        ClaudeClient::with_auth_headers(
            Default::default(),
            messages_url.clone(),
            Default::default(),
        )
        .with_http_layer(layer.clone())
    })
}

pub(crate) fn native(
    openai: Option<OpenAiSource>,
    claude: Option<ClaudeSource>,
    models: Option<&[Model]>,
) -> [Gateway; 2] {
    let codex = match &openai {
        Some(_) => Gateway::new("codex", "this build's Codex client"),
        None => Gateway::new("chatgpt", "Codex on your ChatGPT login"),
    };
    let openai = openai.unwrap_or_else(|| Arc::new(|| OpenAi::builder(OpenAiAuth::chatgpt())));
    let served = models.map_or(Served::Known, |models| {
        Served::Listed(Listing::fixed(models.iter().copied()))
    });
    let claude_gateway = match &claude {
        Some(_) => Gateway::new("claude", "this build's Claude client"),
        None => Gateway::new("claude", "Claude on your Claude Code login"),
    };
    let claude = claude.unwrap_or_else(|| Arc::new(ClaudeClient::default));
    [
        codex.login().codex(openai, served),
        claude_gateway.login().claude(claude),
    ]
}

pub(crate) fn keyed(mut clients: ModelClients, args: &GatewayArgs) -> Result<ModelClients> {
    if clients.supplies_openai() {
        return Ok(clients);
    }
    for kind in [
        GatewayArg::Openrouter,
        GatewayArg::Vercel,
        GatewayArg::Cloudflare,
        GatewayArg::CloudflareGateway,
    ] {
        let explicit = args.gateway == Some(kind);
        if let Some(gateway) = routed(kind, args, explicit)? {
            if explicit {
                clients = clients.prefer(gateway.name.clone());
            }
            clients = clients.gateway(gateway);
        }
    }
    Ok(clients)
}

fn env_secret(name: &'static str) -> Option<(&'static str, String)> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| (name, value))
}

pub(crate) const fn kind_name(kind: GatewayArg) -> &'static str {
    match kind {
        GatewayArg::Openrouter => "openrouter",
        GatewayArg::Vercel => "vercel",
        GatewayArg::Cloudflare => "cloudflare",
        GatewayArg::CloudflareGateway => "cloudflare-gateway",
        GatewayArg::Tempo => "tempo",
        GatewayArg::TempoGateway => "tempo-gateway",
        GatewayArg::Boundless => "boundless",
    }
}

fn routed(kind: GatewayArg, args: &GatewayArgs, explicit: bool) -> Result<Option<Gateway>> {
    let name = kind_name(kind);
    let present = |value: &Option<String>| value.clone().filter(|value| !value.trim().is_empty());
    let missing = |names: &str| -> Result<Option<Gateway>> {
        match explicit {
            true => Err(format!("--gateway {name} needs {names}").into()),
            false => Ok(None),
        }
    };
    let (mut route, auth, credential) = match kind {
        GatewayArg::Openrouter => {
            let Some((credential, key)) = env_secret("OPENROUTER_API_KEY") else {
                return missing("OPENROUTER_API_KEY in the environment");
            };
            (
                ResponsesGateway::openrouter(),
                OpenAiAuth::api_key(key),
                credential,
            )
        }
        GatewayArg::Vercel => {
            let Some((credential, key)) =
                env_secret("AI_GATEWAY_API_KEY").or_else(|| env_secret("VERCEL_OIDC_TOKEN"))
            else {
                return missing("AI_GATEWAY_API_KEY or VERCEL_OIDC_TOKEN in the environment");
            };
            (
                ResponsesGateway::vercel(),
                OpenAiAuth::api_key(key),
                credential,
            )
        }
        GatewayArg::Cloudflare => {
            let Some((credential, key)) = env_secret("CLOUDFLARE_API_TOKEN") else {
                return missing("CLOUDFLARE_API_TOKEN in the environment");
            };
            let Some(account) = present(&args.cloudflare_account) else {
                return missing("--cloudflare-account or CLOUDFLARE_ACCOUNT_ID");
            };
            let mut route = ResponsesGateway::cloudflare(account);
            if let Some(gateway) = present(&args.cloudflare_gateway) {
                route = route.gateway_id(gateway);
            }
            (route, OpenAiAuth::api_key(key), credential)
        }
        GatewayArg::CloudflareGateway => {
            let Some(account) = present(&args.cloudflare_account) else {
                return missing("--cloudflare-account or CLOUDFLARE_ACCOUNT_ID");
            };
            let Some(gateway) = present(&args.cloudflare_gateway) else {
                return missing("--cloudflare-gateway or CLOUDFLARE_GATEWAY_ID");
            };
            let mut route = ResponsesGateway::cloudflare_gateway(account, gateway);
            let gateway_token = env_secret("CF_AIG_TOKEN");
            if let Some((_, token)) = &gateway_token {
                route = route.gateway_token(token.clone());
            }
            match (
                env_secret("OPENAI_API_KEY"),
                gateway_token,
                env_secret("CLOUDFLARE_API_TOKEN"),
            ) {
                (Some((credential, key)), _, _) => (route, OpenAiAuth::api_key(key), credential),
                (None, Some(_), _) => (route, OpenAiAuth::gateway_managed(), "CF_AIG_TOKEN"),
                (None, None, Some((credential, key))) => {
                    (route, OpenAiAuth::api_key(key), credential)
                }
                (None, None, None) => {
                    return missing(
                        "OPENAI_API_KEY, CF_AIG_TOKEN or CLOUDFLARE_API_TOKEN in the environment",
                    );
                }
            }
        }
        GatewayArg::Tempo | GatewayArg::TempoGateway | GatewayArg::Boundless => return Ok(None),
    };
    if explicit && let Some(url) = &args.gateway_base_url {
        route = route.base_url(url.clone());
    }
    let about = format!(
        "Codex at {} · credential {credential}",
        route.api_base_url()
    );
    let openai = {
        let route = route.clone();
        Arc::new(move || OpenAi::builder(auth.clone()).responses_gateway(route.clone()))
    };
    Ok(Some(
        Gateway::new(name, about).codex(openai, Served::Routed(route)),
    ))
}

pub(crate) fn parse(gateways: &[Gateway], value: &str) -> Option<ModelSelection> {
    let value = value.trim();
    Model::known(value)
        .map(ModelSelection::Codex)
        .or_else(|| {
            value
                .parse::<ClaudeModel>()
                .ok()
                .map(ModelSelection::Claude)
        })
        .or_else(|| {
            gateways
                .iter()
                .filter_map(Gateway::listing)
                .find_map(|listing| listing.find(value))
                .map(ModelSelection::Codex)
        })
}

pub(crate) fn unknown(gateways: &[Gateway], value: &str) -> crate::host::Error {
    let known = gateways
        .iter()
        .filter_map(|gateway| {
            let (models, note) = gateway.models();
            let models = models
                .iter()
                .map(ToString::to_string)
                .chain(note)
                .collect::<Vec<_>>()
                .join(", ");
            (!models.is_empty()).then(|| format!("{}: {models}", gateway.name))
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("unknown model {value:?}; known models: {known}").into()
}

pub(crate) async fn listed(gateways: &[Gateway]) {
    futures_util::future::join_all(
        gateways
            .iter()
            .filter_map(Gateway::listing)
            .map(|listing| listing.load()),
    )
    .await;
}

pub(crate) fn listings(gateways: &[Gateway]) -> Vec<Arc<Listing>> {
    gateways
        .iter()
        .filter_map(Gateway::listing)
        .cloned()
        .collect()
}

struct Route {
    gateway: Gateway,
    openai: Option<OpenAi>,
    claude: Option<ClaudeClient>,
}

struct Table {
    routes: Vec<Route>,
    gateways: Vec<Gateway>,
    preferred: Option<usize>,
    chosen: Atom<Vec<(ModelSelection, usize)>>,
    sessions: Atom<HashMap<String, usize>>,
}

#[derive(Clone)]
pub(crate) struct Routes(Arc<Table>);

impl Routes {
    pub(crate) fn build(
        gateways: Vec<Gateway>,
        preferred: Option<&str>,
        configure: impl Fn(&Gateway, OpenAiBuilder) -> OpenAiBuilder,
    ) -> Result<Self> {
        let preferred = match preferred {
            Some(name) => Some(
                gateways
                    .iter()
                    .position(|gateway| gateway.name == name)
                    .ok_or_else(|| format!("no gateway is named {name}"))?,
            ),
            None => None,
        };
        let mut routes = Vec::new();
        for gateway in &gateways {
            let openai = match &gateway.codex {
                Some((openai, _)) => Some(
                    configure(gateway, openai())
                        .build()
                        .map_err(|error| format!("the {} gateway: {error}", gateway.name))?,
                ),
                None => None,
            };
            routes.push(Route {
                gateway: gateway.clone(),
                openai,
                claude: gateway.claude.as_ref().map(|claude| claude()),
            });
        }
        Ok(Self(Arc::new(Table {
            routes,
            gateways,
            preferred,
            chosen: Atom::default(),
            sessions: Atom::default(),
        })))
    }

    pub(crate) fn gateways(&self) -> &[Gateway] {
        &self.0.gateways
    }

    pub(crate) fn gateway(&self, index: usize) -> &Gateway {
        &self.0.routes[index].gateway
    }

    pub(crate) fn preferred(&self) -> Option<usize> {
        self.0.preferred
    }

    pub(crate) fn find(&self, name: &str) -> Option<usize> {
        self.0
            .gateways
            .iter()
            .position(|gateway| gateway.name == name)
    }

    pub(crate) fn default_openai(&self) -> Option<OpenAi> {
        self.0.routes.iter().find_map(|route| route.openai.clone())
    }

    pub(crate) fn default_claude(&self) -> Option<ClaudeClient> {
        self.0.routes.iter().find_map(|route| route.claude.clone())
    }

    pub(crate) fn openai(&self, request: &HarnessRequest) -> Option<OpenAi> {
        self.0.routes[self.route(request)?].openai.clone()
    }

    pub(crate) fn claude(&self, request: &HarnessRequest) -> Option<ClaudeClient> {
        self.0.routes[self.route(request)?].claude.clone()
    }

    pub(crate) fn of_session(&self, session: &str) -> Option<usize> {
        self.0
            .sessions
            .read(|sessions| sessions.get(session).copied())
    }

    pub(crate) fn choose(&self, model: &ModelSelection, index: usize) {
        self.0.chosen.update(|chosen| {
            chosen.retain(|(existing, _)| existing != model);
            chosen.push((model.clone(), index));
        });
    }

    fn chosen(&self, model: &ModelSelection) -> Option<usize> {
        self.0.chosen.read(|chosen| {
            chosen
                .iter()
                .find(|(existing, _)| existing == model)
                .map(|(_, index)| *index)
        })
    }

    pub(crate) fn resolve(&self, model: &ModelSelection, hint: Option<usize>) -> Option<usize> {
        let serves = |index: &usize| self.0.gateways[*index].serves(model);
        hint.filter(serves)
            .or_else(|| self.chosen(model).filter(serves))
            .or_else(|| self.0.preferred.filter(serves))
            .or_else(|| (0..self.0.gateways.len()).find(serves))
    }

    pub(crate) fn parse(
        &self,
        value: &str,
        hint: Option<usize>,
    ) -> Result<(ModelSelection, usize)> {
        let model =
            parse(&self.0.gateways, value).ok_or_else(|| unknown(&self.0.gateways, value))?;
        let index = self
            .resolve(&model, hint)
            .ok_or_else(|| unknown(&self.0.gateways, value))?;
        Ok((model, index))
    }

    fn route(&self, request: &HarnessRequest) -> Option<usize> {
        let session = request
            .durable_state
            .as_ref()
            .map(|state| state.state_id().to_owned());
        if let Some(index) = session
            .as_deref()
            .and_then(|session| self.of_session(session))
        {
            return Some(index);
        }
        let parent = request
            .parent
            .as_ref()
            .and_then(|parent| self.of_session(parent.session_id()));
        let index = self.resolve(&request.model, parent)?;
        if let Some(session) = session {
            self.0.sessions.update(|sessions| {
                sessions.insert(session.clone(), index);
            });
        }
        Some(index)
    }
}
