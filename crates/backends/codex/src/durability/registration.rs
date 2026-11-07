use std::sync::Arc;

use nanocode_agent_core::{Agent, AgentEvents, BackendRequest, Result, backend::BackendFuture};
use nanocode_durability::{
    CheckpointFormat, DurableAgentExt as _, DurableBackendRegistration, DurableSession,
};
use nanocode_platform::MaybeSend;
use tower::Service;

use crate::{
    CodexBuilder, OpenAi,
    client::{
        __private::ResponsesServiceFactory,
        ResponseError,
        tower::{ResponsesAttempt, ResponsesServiceResponse},
    },
    durability::CodexCheckpointFormat,
};

impl<F> DurableBackendRegistration for CodexBuilder<F>
where
    F: ResponsesServiceFactory + Send + Sync + 'static,
    F::Service:
        Service<ResponsesAttempt, Response = ResponsesServiceResponse> + MaybeSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Error: Into<ResponseError> + MaybeSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Future: MaybeSend,
{
    fn checkpoint_format(&self) -> Option<Arc<dyn CheckpointFormat>> {
        Some(Arc::new(CodexCheckpointFormat))
    }

    fn build_durable(
        &self,
        request: BackendRequest,
        state: DurableSession,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        let prepared = self.clone().prepare(request);
        Box::pin(async move {
            let builder = prepared?;
            let session_id = state
                .state_id()
                .parse::<crate::session::SessionId>()
                .map_err(|error| {
                    nanocode_agent_core::NanocodeError::InvalidRequest(format!(
                        "registered durable Codex state ID must be a UUID: {error}"
                    ))
                })?;
            builder
                .session_id(session_id)
                .durability(state)
                .await?
                .build()
        })
    }
}

impl<F> DurableBackendRegistration for OpenAi<F>
where
    F: ResponsesServiceFactory + Send + Sync + 'static,
    F::Service:
        Service<ResponsesAttempt, Response = ResponsesServiceResponse> + MaybeSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Error: Into<ResponseError> + MaybeSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Future: MaybeSend,
{
    fn checkpoint_format(&self) -> Option<Arc<dyn CheckpointFormat>> {
        Some(Arc::new(CodexCheckpointFormat))
    }

    fn build_durable(
        &self,
        request: BackendRequest,
        state: DurableSession,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        Agent::builder(self.clone()).build_durable(request, state)
    }
}
