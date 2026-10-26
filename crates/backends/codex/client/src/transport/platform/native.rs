use std::{future::Future, pin::Pin};

use crate::client::{
    ModelConfig, OpenAiAuthSnapshot, ResponsesError,
    http::ResponsesHttp,
    socket::{ConnectionMetadata, ResponsesSocket},
    tower::{ResponsesServiceError, ResponsesServiceResponse},
};

pub(crate) type ServiceFuture =
    Pin<Box<dyn Future<Output = Result<ResponsesServiceResponse, ResponsesServiceError>> + Send>>;

#[derive(Clone)]
pub(crate) struct ServicePlatform {
    http: ResponsesHttp,
}

impl ServicePlatform {
    pub(crate) fn with_http_client(config: &ModelConfig, client: reqwest::Client) -> Self {
        Self {
            http: ResponsesHttp::new(
                client,
                config.host_transport.clone(),
                config.chat_gateway.clone(),
                config.responses_gateway.clone(),
                config.http_layer.clone(),
            ),
        }
    }

    pub(crate) const fn http(&self) -> &ResponsesHttp {
        &self.http
    }
}

pub(crate) async fn connect_socket(
    _platform: &ServicePlatform,
    config: &ModelConfig,
    auth: &OpenAiAuthSnapshot,
    session_id: &str,
    thread_id: &str,
    turn_state: Option<&str>,
) -> Result<(ResponsesSocket, ConnectionMetadata), ResponsesError> {
    ResponsesSocket::connect(
        &config.websocket_url,
        auth,
        session_id,
        thread_id,
        turn_state,
    )
    .await
}
