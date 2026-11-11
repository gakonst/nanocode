use crate::client::{OpenAiAuthSnapshot, ResponsesError, monotonic_now_ns};

use super::{HostConnectRequest, HostConnection, HostError, HostMessage, HostTransport};
use crate::client::transport::{EncodedRequest, SendOutcome, wire::turn_state_from_event};

pub(crate) use crate::client::transport::wire::decode_event;

pub(crate) struct ConnectionMetadata {
    pub status: u16,
    pub request_id: Option<String>,
    pub server_model: Option<String>,
    pub reasoning_included: bool,
    pub turn_state: Option<String>,
}

pub(crate) struct ResponsesSocket {
    connection: Box<dyn HostConnection>,
    turn_state: Option<String>,
}

pub(crate) struct ReceivedText {
    pub text: String,
    pub received_ns: u64,
    pub retry_receipt: crate::client::transport::RetryReceipt,
}

impl ResponsesSocket {
    pub(crate) async fn connect(
        host: &dyn HostTransport,
        endpoint: &str,
        auth: &OpenAiAuthSnapshot,
        session_id: &str,
        thread_id: &str,
        turn_state: Option<&str>,
    ) -> Result<(Self, ConnectionMetadata), ResponsesError> {
        let request = HostConnectRequest::new_with_thread_id(
            endpoint,
            auth.bearer(),
            auth.account_id(),
            auth.is_fedramp(),
            session_id,
            thread_id,
            turn_state,
        );
        let (connection, metadata) = host
            .connect(request)
            .await
            .map_err(|error| match error {
                HostError::HandshakeRejected {
                    status,
                    body,
                    retry_after,
                    should_retry,
                } => ResponsesError::HandshakeRejected {
                    status,
                    body,
                    retry_after,
                    should_retry,
                },
                error => ResponsesError::Handshake {
                    reconnectable: error.is_reconnectable(),
                    detail: error.to_string(),
                },
            })?
            .into_parts();
        let metadata = ConnectionMetadata {
            status: metadata.status,
            request_id: metadata.request_id,
            server_model: metadata.server_model,
            reasoning_included: metadata.reasoning_included,
            turn_state: metadata.turn_state,
        };
        Ok((
            Self {
                connection,
                turn_state: metadata.turn_state.clone(),
            },
            metadata,
        ))
    }

    pub(crate) async fn send(
        &self,
        request: EncodedRequest,
    ) -> Result<SendOutcome, ResponsesError> {
        if self.connection.is_closed() {
            return Ok(SendOutcome::Disconnected);
        }
        self.connection
            .send(request.json())
            .await
            .map_err(|error| {
                let reconnectable = error.is_reconnectable();
                ResponsesError::Send {
                    reconnectable,
                    detail: error.to_string(),
                }
            })
            .map(|()| SendOutcome::Sent)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.connection.is_closed()
    }

    pub(crate) async fn next_text(&mut self) -> Result<ReceivedText, ResponsesError> {
        match self.connection.next().await.map_err(|error| {
            let reconnectable = error.is_reconnectable();
            ResponsesError::Receive {
                reconnectable,
                detail: error.to_string(),
            }
        })? {
            HostMessage::Text(text) => {
                self.capture_turn_state(&text);
                Ok(ReceivedText {
                    text,
                    received_ns: monotonic_now_ns(),
                    retry_receipt: crate::client::transport::RetryReceipt::now(),
                })
            }
            HostMessage::Closed { detail } => Err(ResponsesError::Closed { detail }),
            HostMessage::Binary => Err(ResponsesError::UnexpectedBinary),
        }
    }

    pub(crate) const fn try_next_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError> {
        Ok(None)
    }

    pub(crate) fn turn_state(&self) -> Option<&str> {
        self.turn_state.as_deref()
    }

    pub(crate) fn reset_turn_state(&mut self) {
        self.turn_state = None;
    }

    fn capture_turn_state(&mut self, text: &str) {
        if self.turn_state.is_none() {
            self.turn_state = turn_state_from_event(text);
        }
    }
}

impl Drop for ResponsesSocket {
    fn drop(&mut self) {
        self.connection.close();
    }
}
