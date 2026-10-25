use std::{fmt, sync::Arc};

use serde::Serialize;
use serde_json::Value;

use crate::client::Model;

const OPENROUTER_API_BASE_URL: &str = "https://openrouter.ai/api/v1";
const VERCEL_API_BASE_URL: &str = "https://ai-gateway.vercel.sh/v1";
const CLOUDFLARE_API_BASE_URL: &str = "https://api.cloudflare.com/client/v4/accounts";
const CLOUDFLARE_GATEWAY_BASE_URL: &str = "https://gateway.ai.cloudflare.com/v1";
const OPENAI_VENDOR: &str = "openai";
const ZAI_VENDOR: &str = "zai";
const MOONSHOT_VENDOR: &str = "moonshotai";
const XIAOMI_VENDOR: &str = "xiaomi";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Provider {
    OpenRouter,
    Vercel,
    Cloudflare,
    CloudflareGateway,
}

#[derive(Clone)]
pub struct GatewayHeader {
    name: &'static str,
    value: Arc<str>,
    secret: bool,
}

impl GatewayHeader {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    pub(crate) fn plain(name: &'static str, value: &str) -> Self {
        Self {
            name,
            value: Arc::from(value),
            secret: false,
        }
    }

    pub(crate) const fn secret_value(&self) -> Option<&Arc<str>> {
        if self.secret { Some(&self.value) } else { None }
    }
}

impl fmt::Debug for GatewayHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value: &dyn fmt::Debug = if self.secret {
            &"[redacted]"
        } else {
            &self.value
        };
        formatter
            .debug_struct("GatewayHeader")
            .field("name", &self.name)
            .field("value", value)
            .finish()
    }
}

#[derive(Clone)]
pub struct ResponsesGateway {
    provider: Provider,
    api_base_url: Arc<str>,
    path_segments: Vec<Arc<str>>,
    headers: Vec<GatewayHeader>,
    routing: Option<Arc<Value>>,
    misuse: Option<&'static str>,
}

#[derive(Serialize)]
pub(crate) struct VercelProviderOptions<'a> {
    gateway: &'a Value,
}

impl ResponsesGateway {
    fn new(provider: Provider, api_base_url: String, path_segments: Vec<Arc<str>>) -> Self {
        Self {
            provider,
            api_base_url: Arc::from(api_base_url),
            path_segments,
            headers: Vec::new(),
            routing: None,
            misuse: None,
        }
    }

    #[must_use]
    pub fn openrouter() -> Self {
        Self::new(
            Provider::OpenRouter,
            OPENROUTER_API_BASE_URL.to_owned(),
            Vec::new(),
        )
    }

    #[must_use]
    pub fn vercel() -> Self {
        Self::new(Provider::Vercel, VERCEL_API_BASE_URL.to_owned(), Vec::new())
    }

    #[must_use]
    pub fn cloudflare(account_id: impl Into<String>) -> Self {
        let account_id: Arc<str> = Arc::from(account_id.into().trim());
        Self::new(
            Provider::Cloudflare,
            format!("{CLOUDFLARE_API_BASE_URL}/{account_id}/ai/v1"),
            vec![account_id],
        )
    }

    #[must_use]
    pub fn cloudflare_gateway(
        account_id: impl Into<String>,
        gateway_id: impl Into<String>,
    ) -> Self {
        let account_id: Arc<str> = Arc::from(account_id.into().trim());
        let gateway_id: Arc<str> = Arc::from(gateway_id.into().trim());
        Self::new(
            Provider::CloudflareGateway,
            format!("{CLOUDFLARE_GATEWAY_BASE_URL}/{account_id}/{gateway_id}/openai"),
            vec![account_id, gateway_id],
        )
    }

    #[must_use]
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = Arc::from(url.into().trim());
        self
    }

    #[must_use]
    pub fn gateway_id(self, gateway_id: impl Into<String>) -> Self {
        self.header_for(
            Provider::Cloudflare,
            "cf-aig-gateway-id",
            gateway_id.into(),
            false,
            "gateway_id applies to the Cloudflare unified API; cloudflare_gateway already names its gateway in the URL",
        )
    }

    #[must_use]
    pub fn gateway_token(self, token: impl Into<String>) -> Self {
        let token = token.into();
        self.header_for(
            Provider::CloudflareGateway,
            "cf-aig-authorization",
            format!("Bearer {}", token.trim()),
            true,
            "gateway_token applies to Cloudflare provider-native gateway endpoints; the unified API authenticates with the bearer token",
        )
    }

    #[must_use]
    pub fn byok_alias(self, alias: impl Into<String>) -> Self {
        self.header_for(
            Provider::CloudflareGateway,
            "cf-aig-byok-alias",
            alias.into(),
            false,
            "byok_alias applies to Cloudflare provider-native gateway endpoints",
        )
    }

    #[must_use]
    pub fn app_title(self, title: impl Into<String>) -> Self {
        self.header_for(
            Provider::OpenRouter,
            "X-OpenRouter-Title",
            title.into(),
            false,
            "app_title applies to OpenRouter",
        )
    }

    #[must_use]
    pub fn app_url(self, url: impl Into<String>) -> Self {
        self.header_for(
            Provider::OpenRouter,
            "HTTP-Referer",
            url.into(),
            false,
            "app_url applies to OpenRouter",
        )
    }

    #[must_use]
    pub fn provider_routing(mut self, routing: Value) -> Self {
        if matches!(self.provider, Provider::OpenRouter | Provider::Vercel) {
            self.routing = Some(Arc::new(routing));
        } else {
            self.misuse.get_or_insert(
                "provider_routing applies to OpenRouter provider preferences and Vercel providerOptions.gateway",
            );
        }
        self
    }

    fn header_for(
        mut self,
        provider: Provider,
        name: &'static str,
        value: String,
        secret: bool,
        misuse: &'static str,
    ) -> Self {
        if self.provider == provider {
            self.headers.retain(|header| header.name != name);
            self.headers.push(GatewayHeader {
                name,
                value: Arc::from(value.trim()),
                secret,
            });
        } else {
            self.misuse.get_or_insert(misuse);
        }
        self
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self.provider {
            Provider::OpenRouter => "openrouter",
            Provider::Vercel => "vercel",
            Provider::Cloudflare => "cloudflare",
            Provider::CloudflareGateway => "cloudflare-gateway",
        }
    }

    #[must_use]
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    #[must_use]
    pub fn headers(&self) -> &[GatewayHeader] {
        &self.headers
    }

    #[must_use]
    pub fn provider_routing_value(&self) -> Option<&Value> {
        self.routing.as_deref()
    }

    #[must_use]
    pub const fn model_id_prefix(&self) -> Option<&'static str> {
        match self.provider {
            Provider::OpenRouter | Provider::Vercel | Provider::Cloudflare => Some(OPENAI_VENDOR),
            Provider::CloudflareGateway => None,
        }
    }

    #[must_use]
    pub const fn model_id_prefix_for(&self, model: Model) -> Option<&'static str> {
        match (self.provider, model) {
            (Provider::Vercel, Model::Glm53) => Some(ZAI_VENDOR),
            (Provider::Vercel, Model::Kimi) => Some(MOONSHOT_VENDOR),
            (Provider::Vercel, Model::Mimo) => Some(XIAOMI_VENDOR),
            _ => self.model_id_prefix(),
        }
    }

    #[must_use]
    pub const fn sends_custom_tools_as_functions(&self, model: Model) -> bool {
        matches!(
            model,
            Model::Glm53 | Model::Kimi | Model::Mimo | Model::Custom(_)
        )
    }

    #[must_use]
    pub const fn serves(&self, model: Model) -> bool {
        matches!(
            model,
            Model::Sol | Model::Luna | Model::Astra | Model::DaybreakBlue | Model::Custom(_)
        ) || matches!(self.provider, Provider::Vercel)
            && matches!(model, Model::Glm53 | Model::Kimi | Model::Mimo)
    }

    #[must_use]
    pub fn authenticates_requests(&self) -> bool {
        matches!(self.provider, Provider::CloudflareGateway)
            && self
                .headers
                .iter()
                .any(|header| header.name == "cf-aig-authorization")
    }

    #[must_use]
    pub const fn supports_inline_compaction(&self) -> bool {
        matches!(self.provider, Provider::CloudflareGateway)
    }

    #[must_use]
    pub const fn supports_websocket(&self) -> bool {
        matches!(self.provider, Provider::Vercel)
    }

    #[must_use]
    pub const fn supports_stored_responses(&self) -> bool {
        self.stored_responses_rejection().is_none()
    }

    pub(crate) const fn stored_responses_rejection(&self) -> Option<&'static str> {
        match self.provider {
            Provider::OpenRouter => {
                Some("OpenRouter's Responses API is stateless and rejects store: true")
            }
            Provider::Cloudflare => Some(
                "Cloudflare's unified Responses API replaces response IDs, so previous_response_id cannot continue them; use store: false",
            ),
            Provider::Vercel | Provider::CloudflareGateway => None,
        }
    }

    #[must_use]
    pub fn websocket_url(&self) -> Option<String> {
        if !self.supports_websocket() {
            return None;
        }
        let base = self.api_base_url.trim_end_matches('/');
        let socket = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            base.to_owned()
        };
        Some(format!("{socket}/responses"))
    }

    pub(crate) fn openrouter_provider(&self) -> Option<&Value> {
        matches!(self.provider, Provider::OpenRouter)
            .then_some(self.routing.as_deref())
            .flatten()
    }

    pub(crate) fn vercel_provider_options(&self) -> Option<VercelProviderOptions<'_>> {
        matches!(self.provider, Provider::Vercel)
            .then_some(self.routing.as_deref())
            .flatten()
            .map(|gateway| VercelProviderOptions { gateway })
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if let Some(misuse) = self.misuse {
            return Err(misuse);
        }
        if self.path_segments.iter().any(|segment| {
            matches!(segment.as_ref(), "" | "." | "..")
                || segment.chars().any(|character| {
                    character.is_whitespace()
                        || character.is_control()
                        || matches!(character, '/' | '\\' | '?' | '#' | '%' | '@')
                })
        }) {
            return Err(
                "Cloudflare account and gateway IDs must be single non-empty URL path segments",
            );
        }
        validate_base_url(&self.api_base_url)?;
        if self.headers.iter().any(|header| {
            header.value.is_empty() || http::HeaderValue::from_str(&header.value).is_err()
        }) {
            return Err("gateway header values must be non-empty visible ASCII");
        }
        if self.headers.iter().any(|header| {
            header.name == "cf-aig-authorization" && header.value.as_ref() == "Bearer"
        }) {
            return Err("the Cloudflare gateway token must not be empty");
        }
        if self
            .routing
            .as_deref()
            .is_some_and(|routing| !routing.is_object())
        {
            return Err("gateway provider routing must be a JSON object");
        }
        Ok(())
    }
}

pub(crate) fn validate_base_url(url: &str) -> Result<(), &'static str> {
    let uri: http::Uri = url
        .parse()
        .map_err(|_| "the gateway base URL must be an absolute http or https URL")?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return Err("the gateway base URL must use http or https");
    }
    let Some(authority) = uri.authority() else {
        return Err("the gateway base URL must include a host");
    };
    if authority.as_str().contains('@') {
        return Err(
            "the gateway base URL must not embed credentials; use the API key or gateway headers",
        );
    }
    if authority.host().is_empty() {
        return Err("the gateway base URL must include a host");
    }
    if uri.query().is_some() || url.contains('#') {
        return Err(
            "the gateway base URL must not contain a query or fragment because request paths are appended to it",
        );
    }
    if uri
        .path()
        .split('/')
        .any(|segment| matches!(segment, "." | ".."))
    {
        return Err("the gateway base URL must not contain dot path segments");
    }
    Ok(())
}

fn displayed_base_url(url: &str) -> &str {
    if url.contains('@') { "[redacted]" } else { url }
}

impl fmt::Debug for ResponsesGateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponsesGateway")
            .field("name", &self.name())
            .field("api_base_url", &displayed_base_url(&self.api_base_url))
            .field("headers", &self.headers)
            .field("provider_routing", &self.routing)
            .finish_non_exhaustive()
    }
}
