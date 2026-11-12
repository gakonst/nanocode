use crate::agent::AgentSend;
use crate::client::tower::Service;
use crate::client::tower::{ResponsesAttempt, ResponsesServiceResponse};
use crate::client::{__private::ResponsesServiceFactory, Model, ResponseError};
use crate::{Agent, AgentEvents, CodexBuilder, NanocodeError, OpenAi, Result};
use nanocode_agent_core::{BackendId, BackendRegistration, BackendRequest};

impl<F> nanocode_agent_core::BackendKind for OpenAi<F> {
    type Model = Model;

    fn backend_id() -> BackendId {
        BackendId::new("codex")
    }

    fn model_id(model: &Self::Model) -> &str {
        model.as_str()
    }
}

impl<F> nanocode_agent_core::BackendKind for CodexBuilder<F> {
    type Model = Model;

    fn backend_id() -> BackendId {
        BackendId::new("codex")
    }

    fn model_id(model: &Self::Model) -> &str {
        model.as_str()
    }
}

impl<F> BackendRegistration for OpenAi<F>
where
    F: ResponsesServiceFactory + Send + Sync + 'static,
    F::Service:
        Service<ResponsesAttempt, Response = ResponsesServiceResponse> + AgentSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Error: Into<ResponseError> + AgentSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Future: AgentSend,
{
    fn id(&self) -> BackendId {
        BackendId::new("codex")
    }

    fn default_model(&self) -> String {
        crate::client::__private::into_openai_parts(self.clone())
            .0
            .model
            .as_str()
            .to_owned()
    }

    fn default_thinking(&self) -> Option<crate::Thinking> {
        Some(
            crate::client::__private::into_openai_parts(self.clone())
                .0
                .thinking,
        )
    }

    fn validate_model(&self, model: &str) -> Result<()> {
        validate_model(model)
    }

    fn build(
        &self,
        request: BackendRequest,
    ) -> nanocode_agent_core::backend::BackendFuture<Result<(Agent, AgentEvents)>> {
        BackendRegistration::build(&Agent::builder(self.clone()), request)
    }
}

impl<F> BackendRegistration for CodexBuilder<F>
where
    F: ResponsesServiceFactory + Send + Sync + 'static,
    F::Service:
        Service<ResponsesAttempt, Response = ResponsesServiceResponse> + AgentSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Error: Into<ResponseError> + AgentSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Future: AgentSend,
{
    fn id(&self) -> BackendId {
        BackendId::new("codex")
    }

    fn default_model(&self) -> String {
        self.config.model.as_str().to_owned()
    }

    fn default_thinking(&self) -> Option<crate::Thinking> {
        Some(self.config.thinking)
    }

    fn validate_model(&self, model: &str) -> Result<()> {
        validate_model(model)
    }

    fn build(
        &self,
        request: BackendRequest,
    ) -> nanocode_agent_core::backend::BackendFuture<Result<(Agent, AgentEvents)>> {
        let prepared = self.clone().prepare(request);
        Box::pin(async move { prepared?.build() })
    }
}

fn validate_model(model: &str) -> Result<()> {
    model
        .parse::<Model>()
        .map(|_| ())
        .map_err(NanocodeError::InvalidRequest)
}

impl<F> CodexBuilder<F> {
    pub fn prepare(self, request: BackendRequest) -> Result<Self> {
        if request.model.backend != BackendId::new("codex") {
            return Err(NanocodeError::InvalidRequest(
                "Codex registration requires the Codex backend".into(),
            ));
        }
        let model = request
            .model
            .model
            .parse::<Model>()
            .map_err(NanocodeError::InvalidRequest)?;
        let mut builder = self
            .runtime_tools(request.tools)
            .model(model)
            .workspace(request.workspace)
            .spawn_factory(request.spawn_factory)
            .host_context(request.host_context);
        builder = builder.reasoning_mode(
            nanocode_agent_core::ModelSelection::Codex(model)
                .capabilities(nanocode_agent_core::ModelTransport::Native)
                .normalize_reasoning_mode(
                    request
                        .options
                        .selected_reasoning_mode()
                        .unwrap_or_default(),
                ),
        );
        if let Some(checkpoint) = request.checkpoint {
            builder = builder.resume(checkpoint)?;
        }
        if let Some(thinking) = request.thinking.or(request.options.selected_thinking()) {
            builder = builder.thinking(thinking);
        }
        if let Some(instructions) = request.instructions {
            builder = builder.additional_instructions(instructions);
        }
        if request.options.stateless_http {
            builder = builder.stateless_http();
        }
        Ok(builder)
    }
}
