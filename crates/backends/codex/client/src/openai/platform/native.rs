use std::{
    num::NonZeroU32,
    sync::{Arc, OnceLock},
};

use crate::client::{
    DefaultResponsesService, ModelConfig, OpenAiError, ResponsesService, ResponsesTransport,
};

#[derive(Clone, Default)]
pub(crate) struct FactoryPlatform {
    http_client: Arc<OnceLock<reqwest::Client>>,
}

impl FactoryPlatform {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_http_client(&mut self, client: reqwest::Client) {
        self.http_client = Arc::new(OnceLock::from(client));
    }

    pub(crate) fn validate_config(&self, config: &ModelConfig) -> Result<(), OpenAiError> {
        if config.http_layer.is_some() {
            if config.host_transport.is_some() || config.chat_gateway.is_some() {
                return Err(OpenAiError::InvalidConfiguration {
                    detail: "an HTTP layer wraps the native Responses HTTPS transport; remove the host transport or Chat Completions gateway",
                });
            }
            if !matches!(config.responses_transport, ResponsesTransport::Https) {
                return Err(OpenAiError::InvalidConfiguration {
                    detail: "an HTTP layer wraps Responses HTTPS; select the HTTPS Responses transport",
                });
            }
        }
        if config.host_transport.is_none() {
            return Ok(());
        }
        if !matches!(config.responses_transport, ResponsesTransport::Https) {
            return Err(OpenAiError::InvalidConfiguration {
                detail: "native Responses host transports serve HTTPS; select the HTTPS Responses transport",
            });
        }
        Ok(())
    }

    pub(crate) fn make(
        &self,
        config: Arc<ModelConfig>,
        max_attempts: NonZeroU32,
    ) -> DefaultResponsesService {
        ResponsesService::standard_with_http_client_and_max_attempts(
            config,
            self.http_client.get_or_init(reqwest::Client::new).clone(),
            max_attempts,
        )
    }
}
