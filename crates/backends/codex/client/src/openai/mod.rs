use std::{num::NonZeroU32, sync::Arc};

use crate::client::call::Layer;

mod config;
mod platform;

pub(crate) use config::prefixed_model_id;

use crate::client::{
    DefaultResponsesService, Model, OpenAiAuthError, OpenAiAuthMode, ResponsesHistory,
    ResponsesRetryPolicy, ResponsesTransport, transport::ResponsesGateway,
};

pub use config::CyberAccessProgram;
#[doc(hidden)]
pub use config::ModelConfig;
pub use config::ModelConfig as ResponsesServiceConfig;

#[derive(Clone)]
pub struct StandardServiceFactory {
    max_attempts: NonZeroU32,
    platform: platform::FactoryPlatform,
}

impl StandardServiceFactory {
    #[doc(hidden)]
    pub const fn set_max_attempts(&mut self, max_attempts: NonZeroU32) {
        self.max_attempts = max_attempts;
    }

    #[cfg(not(target_family = "wasm"))]
    #[doc(hidden)]
    pub fn set_http_client(&mut self, client: nanocode_http::Client) {
        self.platform.set_http_client(client);
    }
}

impl Default for StandardServiceFactory {
    fn default() -> Self {
        #[cfg(not(target_family = "wasm"))]
        crate::client::transport::install_default_rustls_crypto_provider();
        Self {
            max_attempts: ResponsesRetryPolicy::DEFAULT_MAX_ATTEMPTS,
            platform: platform::FactoryPlatform::new(),
        }
    }
}

pub struct CallerServiceFactory<M> {
    make: Arc<M>,
}

impl<M> CallerServiceFactory<M> {
    #[doc(hidden)]
    pub fn new(make: M) -> Self {
        Self {
            make: Arc::new(make),
        }
    }
}

impl<M> Clone for CallerServiceFactory<M> {
    fn clone(&self) -> Self {
        Self {
            make: Arc::clone(&self.make),
        }
    }
}

#[derive(Clone)]
pub struct LayeredServiceFactory<F, L> {
    inner: F,
    layer: L,
}

impl<F, L> LayeredServiceFactory<F, L> {
    #[doc(hidden)]
    pub const fn new(inner: F, layer: L) -> Self {
        Self { inner, layer }
    }
}

pub trait ResponsesServiceFactory: Clone {
    type Service;

    fn validate_config(&self, _config: &ResponsesServiceConfig) -> Result<(), OpenAiError> {
        Ok(())
    }

    fn make(&self, config: Arc<ResponsesServiceConfig>) -> Self::Service;
}

impl ResponsesServiceFactory for StandardServiceFactory {
    type Service = DefaultResponsesService;

    fn validate_config(&self, config: &ModelConfig) -> Result<(), OpenAiError> {
        self.platform.validate_config(config)
    }

    fn make(&self, config: Arc<ModelConfig>) -> Self::Service {
        self.platform.make(config, self.max_attempts)
    }
}

impl<M, S> ResponsesServiceFactory for CallerServiceFactory<M>
where
    M: Fn() -> S,
{
    type Service = S;

    fn make(&self, _config: Arc<ModelConfig>) -> Self::Service {
        (self.make)()
    }
}

impl<F, L> ResponsesServiceFactory for LayeredServiceFactory<F, L>
where
    F: ResponsesServiceFactory,
    L: Layer<F::Service> + Clone,
{
    type Service = L::Service;

    fn validate_config(&self, config: &ModelConfig) -> Result<(), OpenAiError> {
        self.inner.validate_config(config)
    }

    fn make(&self, config: Arc<ModelConfig>) -> Self::Service {
        self.layer.layer(self.inner.make(config))
    }
}

#[derive(Debug)]
pub enum OpenAiError {
    Authorization(OpenAiAuthError),

    InvalidConfiguration { detail: &'static str },
}

impl std::fmt::Display for OpenAiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authorization(value) => std::fmt::Display::fmt(value, formatter),
            Self::InvalidConfiguration { detail } => {
                write!(formatter, "invalid OpenAI client configuration: {detail}")
            }
        }
    }
}

impl std::error::Error for OpenAiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Authorization(value) => std::error::Error::source(value),
            _ => None,
        }
    }
}

impl From<OpenAiAuthError> for OpenAiError {
    fn from(source: OpenAiAuthError) -> Self {
        Self::Authorization(source)
    }
}

#[doc(hidden)]
pub fn apply_mode_defaults(config: &mut ModelConfig, mode: OpenAiAuthMode) {
    config.store_responses = false;
    config.responses_history = ResponsesHistory::Incremental;
    mode.default_websocket_url()
        .clone_into(&mut config.websocket_url);
    mode.default_api_base_url()
        .clone_into(&mut config.api_base_url);
}

fn validate_responses_gateway(
    config: &ModelConfig,
    gateway: &ResponsesGateway,
) -> Result<(), OpenAiError> {
    gateway
        .validate()
        .and_then(|()| crate::client::transport::gateway::validate_base_url(&config.api_base_url))
        .map_err(|detail| OpenAiError::InvalidConfiguration { detail })?;
    if config.auth.mode() != OpenAiAuthMode::ApiKey {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "Responses gateways require API-key authentication",
        });
    }
    if config.chat_gateway.is_some() {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "choose either a Chat Completions gateway or a Responses gateway",
        });
    }
    if !gateway.serves(config.model) {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "Responses gateways serve the OpenAI Codex models Sol, Terra, Luna and Astra, and the Vercel AI Gateway also GLM-5.3, Kimi and MiMo; use the Chat Completions gateway for those elsewhere",
        });
    }
    if matches!(config.responses_transport, ResponsesTransport::WebSocket)
        && !gateway.supports_websocket()
    {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "this gateway documents the Responses API over HTTPS only",
        });
    }
    if config.store_responses
        && let Some(detail) = gateway.stored_responses_rejection()
    {
        return Err(OpenAiError::InvalidConfiguration { detail });
    }
    Ok(())
}

#[doc(hidden)]
pub fn validate(config: &ModelConfig) -> Result<(), OpenAiError> {
    config.auth.validate()?;
    #[cfg(not(target_family = "wasm"))]
    let layered = config.http_layer.is_some();
    #[cfg(target_family = "wasm")]
    let layered = false;
    if config.auth.is_gateway_managed()
        && config.host_transport.is_none()
        && !layered
        && !config
            .responses_gateway
            .as_deref()
            .is_some_and(ResponsesGateway::authenticates_requests)
    {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "gateway-managed authentication requires a Responses host transport, an HTTP layer or a Cloudflare provider-native gateway with a gateway token",
        });
    }
    if !config.model.supports_thinking(config.thinking) {
        return Err(OpenAiError::InvalidConfiguration {
            detail: (if config.model == Model::Glm53 {
                "GLM-5.3 requires low, medium, or high reasoning effort"
            } else if config.model == Model::Sol {
                "GPT-6.1 Sol requires low, medium, high, xhigh, or max reasoning effort"
            } else {
                "GPT-6 Astra requires low, medium, high, xhigh, or max reasoning effort"
            }),
        });
    }
    if !config.model.supports_reasoning_mode(config.reasoning_mode) {
        return Err(OpenAiError::InvalidConfiguration {
            detail: (if config.model == Model::Glm53 {
                "GLM-5.3 does not support pro reasoning mode"
            } else {
                "the selected gateway model does not support pro reasoning mode"
            }),
        });
    }
    if config.context_window_tokens == 0 {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "the model context window must be greater than zero",
        });
    }
    if config.websocket_url.trim().is_empty() {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "the Responses WebSocket URL must not be empty",
        });
    }
    if config.api_base_url.trim().is_empty() {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "the OpenAI API base URL must not be empty",
        });
    }
    if let Some(prefix) = config.model_id_prefix.as_deref() {
        if prefix.is_empty()
            || prefix.starts_with('/')
            || prefix.ends_with('/')
            || prefix.split('/').any(str::is_empty)
            || prefix
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "the model ID prefix must contain non-empty path segments without whitespace",
            });
        }
        if config.auth.mode() != OpenAiAuthMode::ApiKey {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "model ID prefixes require API-key authentication",
            });
        }
        if !matches!(config.responses_transport, ResponsesTransport::Https)
            && !config
                .responses_gateway
                .as_deref()
                .is_some_and(ResponsesGateway::supports_websocket)
        {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "model ID prefixes require the HTTPS Responses transport",
            });
        }
    }
    if let Some(gateway) = config.responses_gateway.as_deref() {
        validate_responses_gateway(config, gateway)?;
    }
    if let Some(gateway) = config.chat_gateway.as_deref() {
        if !gateway.api_base_url_is_allowed(&config.api_base_url) {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "the gateway endpoint is fixed; only a loopback test base URL may replace it",
            });
        }
        if !gateway.stall_timeout_is_valid() {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "the gateway stall timeout must be greater than zero",
            });
        }
        if !gateway.wire_model_is_valid() {
            return Err(OpenAiError::InvalidConfiguration {
                detail: gateway.wire_model_rule(),
            });
        }
        if config.auth.mode() != OpenAiAuthMode::ApiKey {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "Chat Completions gateways require API-key authentication",
            });
        }
        if config.model_id_prefix.is_some() {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "Chat Completions gateways choose their own model IDs; remove the model ID prefix",
            });
        }
        if !matches!(config.responses_transport, ResponsesTransport::Https)
            || config.store_responses
            || !matches!(config.responses_history, ResponsesHistory::FullReplay)
        {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "Chat Completions gateways require stateless HTTPS with full history replay",
            });
        }
    }
    if config.auth.mode() == OpenAiAuthMode::ChatGpt && config.store_responses {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "ChatGPT subscription authentication does not support store: true",
        });
    }
    if matches!(config.responses_transport, ResponsesTransport::Https)
        && !config.store_responses
        && matches!(config.responses_history, ResponsesHistory::Incremental)
    {
        return Err(OpenAiError::InvalidConfiguration {
            detail: "HTTPS with store: false requires full client-history replay",
        });
    }
    Ok(())
}
