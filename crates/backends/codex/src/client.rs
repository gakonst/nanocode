use std::{num::NonZeroU32, sync::Arc};

pub use nanocode_backend_codex_client::*;

use nanocode_backend_codex_client::__private::{
    CallerServiceFactory, LayeredServiceFactory, ResponsesServiceFactory, StandardServiceFactory,
    apply_mode_defaults, validate,
};
use nanocode_backend_codex_client::{pricing::ServiceTier, responses::StrictJsonSchema};

#[doc(hidden)]
pub mod __private {
    pub use nanocode_backend_codex_client::__private::*;

    pub fn into_openai_parts<F>(openai: super::OpenAi<F>) -> (ModelConfig, F)
    where
        F: ResponsesServiceFactory,
    {
        openai.into_parts()
    }
}

#[derive(Clone)]
#[cfg_attr(not(target_family = "wasm"), derive(Default))]
pub struct OpenAi<F = StandardServiceFactory> {
    config: ModelConfig,
    factory: F,
}

impl OpenAi<StandardServiceFactory> {
    pub fn new(auth: impl Into<OpenAiAuth>) -> Result<Self, OpenAiError> {
        Self::builder(auth).build()
    }

    #[must_use]
    pub fn builder(auth: impl Into<OpenAiAuth>) -> OpenAiBuilder<StandardServiceFactory> {
        let auth = auth.into();
        let mode = auth.mode();
        let mut config = ModelConfig {
            auth,
            ..ModelConfig::default()
        };
        apply_mode_defaults(&mut config, mode);
        OpenAiBuilder {
            config,
            factory: StandardServiceFactory::default(),
        }
    }
}

impl<F> OpenAi<F> {
    #[must_use]
    pub const fn config(&self) -> &ModelConfig {
        &self.config
    }
}

impl<F> OpenAi<F>
where
    F: ResponsesServiceFactory,
{
    #[must_use]
    pub const fn auth_mode(&self) -> OpenAiAuthMode {
        self.config.auth.mode()
    }

    #[must_use]
    pub fn instructions(&self, instructions: impl Into<Arc<str>>) -> SessionBuilder<F> {
        SessionBuilder::new(
            self.config.clone(),
            self.factory.clone(),
            instructions.into(),
        )
    }

    pub(crate) fn into_parts(self) -> (ModelConfig, F) {
        (self.config, self.factory)
    }
}

#[derive(Clone)]
pub struct OpenAiBuilder<F = StandardServiceFactory> {
    config: ModelConfig,
    factory: F,
}

impl<F> OpenAiBuilder<F> {
    #[must_use]
    pub fn model(mut self, model: Model) -> Self {
        self.config.retarget(model);

        if matches!(
            model,
            Model::Glm53 | Model::Kimi | Model::Mimo | Model::Custom(_)
        ) {
            self.config.responses_transport = ResponsesTransport::Https;
            self.config.responses_history = ResponsesHistory::FullReplay;
            self.config.store_responses = false;
            self.config.websocket_warmup = false;
        }
        if !self.config.thinking_explicit {
            self.config.thinking = model.default_thinking();
        }
        if self.config.context_window_tokens > model.max_context_window_tokens() {
            self.config.context_window_tokens = model.max_context_window_tokens();
        }
        self
    }

    #[must_use]
    pub fn model_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        self.config.model_id_prefix = Some(Arc::from(prefix.trim()));
        self
    }

    #[must_use]
    pub const fn transport(mut self, transport: ResponsesTransport) -> Self {
        self.config.responses_transport = transport;
        if matches!(transport, ResponsesTransport::Https) && !self.config.store_responses {
            self.config.responses_history = ResponsesHistory::FullReplay;
        }
        self
    }

    #[must_use]
    pub const fn websocket_warmup(mut self, enabled: bool) -> Self {
        self.config.websocket_warmup = enabled;
        self
    }

    #[must_use]
    pub const fn raw_api_events(mut self, enabled: bool) -> Self {
        self.config.raw_api_events = enabled;
        self
    }

    #[must_use]
    pub const fn tool_input_deltas(mut self, enabled: bool) -> Self {
        self.config.tool_input_deltas = enabled;
        self
    }

    #[must_use]
    pub const fn history(mut self, history: ResponsesHistory) -> Self {
        self.config.responses_history = history;
        self
    }

    #[must_use]
    pub const fn store(mut self, store: bool) -> Self {
        self.config.store_responses = store;
        if matches!(self.config.responses_transport, ResponsesTransport::Https) {
            self.config.responses_history = if store {
                ResponsesHistory::Incremental
            } else {
                ResponsesHistory::FullReplay
            };
        }
        self
    }

    #[must_use]
    pub fn strict_json_schema(mut self, schema: StrictJsonSchema) -> Self {
        self.config.strict_json_schema = Some(schema);
        self
    }

    #[must_use]
    pub const fn thinking(mut self, thinking: Thinking) -> Self {
        self.config.thinking = thinking;
        self.config.thinking_explicit = true;
        self
    }

    #[must_use]
    pub const fn reasoning_mode(mut self, reasoning_mode: ReasoningMode) -> Self {
        self.config.reasoning_mode = reasoning_mode;
        self
    }

    #[must_use]
    pub const fn fast_mode(self, enabled: bool) -> Self {
        self.service_tier(ServiceTier::from_fast_mode(enabled))
    }

    #[must_use]
    pub const fn service_tier(mut self, service_tier: ServiceTier) -> Self {
        self.config.service_tier = service_tier;
        self
    }

    #[must_use]
    pub const fn cyber_access_program(mut self, program: Option<CyberAccessProgram>) -> Self {
        self.config.cyber_access_program = program;
        self
    }

    #[must_use]
    pub fn model_cyber_access_program(mut self, model: Model, program: CyberAccessProgram) -> Self {
        let configured = &mut self.config.model_cyber_access_programs;
        configured.retain(|(existing, _)| *existing != model);
        configured.push((model, program));
        self
    }

    #[must_use]
    pub const fn context_window_tokens(mut self, tokens: u64) -> Self {
        let maximum = self.config.model.max_context_window_tokens();
        self.config.context_window_tokens = if tokens > maximum { maximum } else { tokens };
        self
    }

    #[must_use]
    pub const fn auto_compact_token_limit(mut self, tokens: u64) -> Self {
        self.config.auto_compact_token_limit = Some(tokens);
        self
    }

    #[must_use]
    pub fn websocket_url(mut self, url: impl Into<String>) -> Self {
        self.config.websocket_url = url.into();
        self
    }

    #[must_use]
    pub fn api_base_url(mut self, url: impl Into<String>) -> Self {
        self.config.api_base_url = url.into();
        self
    }

    #[must_use]
    pub fn chat_gateway(mut self, gateway: crate::client::transport::ChatGateway) -> Self {
        self.config.api_base_url = gateway.default_api_base_url().to_owned();
        self.config.chat_gateway = Some(Arc::new(gateway));
        self.config.responses_transport = ResponsesTransport::Https;
        self.config.responses_history = ResponsesHistory::FullReplay;
        self.config.store_responses = false;
        self.config.websocket_warmup = false;
        self
    }

    #[must_use]
    pub fn responses_gateway(
        mut self,
        gateway: crate::client::transport::ResponsesGateway,
    ) -> Self {
        gateway
            .api_base_url()
            .clone_into(&mut self.config.api_base_url);
        if let Some(websocket_url) = gateway.websocket_url() {
            self.config.websocket_url = websocket_url;
        }
        self.config.model_id_prefix = gateway
            .model_id_prefix_for(self.config.model)
            .map(Arc::from);
        self.config.responses_gateway = Some(Arc::new(gateway));
        self.config.responses_transport = ResponsesTransport::Https;
        self.config.responses_history = ResponsesHistory::FullReplay;
        self.config.store_responses = false;
        self.config.websocket_warmup = false;
        self
    }

    #[must_use]
    pub fn host_transport(
        mut self,
        transport: impl crate::client::transport::host::HostTransport,
    ) -> Self {
        self.config.host_transport = Some(Arc::new(transport));
        self
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn http_layer(mut self, layer: impl crate::client::transport::layer::Layer) -> Self {
        self.config.http_layer = Some(Arc::new(layer));
        self.config.responses_transport = ResponsesTransport::Https;
        self.config.websocket_warmup = false;
        self
    }

    #[must_use]
    pub fn layer<L>(self, layer: L) -> OpenAiBuilder<LayeredServiceFactory<F, L>> {
        OpenAiBuilder {
            config: self.config,
            factory: LayeredServiceFactory::new(self.factory, layer),
        }
    }

    #[must_use]
    pub fn service<M>(self, make: M) -> OpenAiBuilder<CallerServiceFactory<M>> {
        OpenAiBuilder {
            config: self.config,
            factory: CallerServiceFactory::new(make),
        }
    }
}

impl OpenAiBuilder<StandardServiceFactory> {
    #[must_use]
    pub const fn max_attempts(mut self, max_attempts: NonZeroU32) -> Self {
        self.factory.set_max_attempts(max_attempts);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.factory.set_http_client(client);
        self
    }
}

impl<F> OpenAiBuilder<F>
where
    F: ResponsesServiceFactory,
{
    pub fn build(self) -> Result<OpenAi<F>, OpenAiError> {
        #[cfg(not(target_family = "wasm"))]
        crate::client::transport::install_default_rustls_crypto_provider();
        validate(&self.config)?;
        self.factory.validate_config(&self.config)?;
        Ok(OpenAi {
            config: self.config,
            factory: self.factory,
        })
    }
}
