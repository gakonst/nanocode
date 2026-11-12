#[cfg(not(target_family = "wasm"))]
#[path = "native.rs"]
mod platform;

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
#[path = "web.rs"]
mod platform;

pub(crate) use platform::{AgentFactory, AgentSend};
pub(super) use platform::{ServiceFactory, spawn_driver};

use crate::agent::execution::ExecutionFuture;
use crate::client::tower::Service;
use crate::client::{
    ResponseError,
    tower::{ResponsesAttempt, ResponsesClient, ResponsesServiceResponse},
};
use std::task::{Context, Poll, Waker};

pub(crate) type ResponsesClientBox = Box<dyn ResponsesExecutor>;

pub(crate) trait ResponsesExecutor: AgentSend {
    fn execute(
        &mut self,
        request: ResponsesAttempt,
    ) -> ExecutionFuture<'_, std::result::Result<ResponsesServiceResponse, ResponseError>>;

    fn preconnect(&mut self, request: ResponsesAttempt) -> Option<ExecutionFuture<'static, ()>>;
}

impl<S> ResponsesExecutor for ResponsesClient<S>
where
    S: Service<ResponsesAttempt, Response = ResponsesServiceResponse> + AgentSend + 'static,
    S::Error: Into<ResponseError> + AgentSend + 'static,
    S::Future: AgentSend + 'static,
{
    fn execute(
        &mut self,
        request: ResponsesAttempt,
    ) -> ExecutionFuture<'_, std::result::Result<ResponsesServiceResponse, ResponseError>> {
        Box::pin(async move { Self::execute(self, request).await.map_err(Into::into) })
    }

    fn preconnect(&mut self, request: ResponsesAttempt) -> Option<ExecutionFuture<'static, ()>> {
        let service = self.service_mut();
        if !matches!(
            service.poll_ready(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(()))
        ) {
            return None;
        }
        let connected = service.call(request);
        Some(Box::pin(async move { drop(connected.await) }))
    }
}
