use std::sync::Arc;

use crate::client::call::Service;
use nanocode_platform::MaybeSend;
use nanocode_sync::mpsc;

use crate::client::{
    EventSink, Model, ModelConfig, ResponseItem, ResponsesAttempt, ResponsesClient,
    ResponsesServiceResponse, Thinking, ToolDefinition, TransportStats,
    openai::{ResponsesServiceFactory, StandardServiceFactory},
    pricing::ServiceTier,
    responses::RequestProfile,
};

use super::{
    context::shared_request_prefix,
    response::{
        CompletedCompaction, Response, ResponseError, ResponseInput, run_compact, run_create,
    },
    state::{ManagedSessionState, SessionId},
};

const RESPONSE_EVENT_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct SessionBuilder<F = StandardServiceFactory> {
    config: ModelConfig,
    factory: F,
    instructions: Arc<str>,
    session_id: Option<SessionId>,
    prompt_cache_key: Option<String>,
    tools: Vec<ToolDefinition>,
}

impl<F> SessionBuilder<F>
where
    F: ResponsesServiceFactory,
{
    #[doc(hidden)]
    pub const fn new(config: ModelConfig, factory: F, instructions: Arc<str>) -> Self {
        Self {
            config,
            factory,
            instructions,
            session_id: None,
            prompt_cache_key: None,
            tools: Vec::new(),
        }
    }

    #[must_use]
    pub const fn session_id(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    #[must_use]
    pub fn prompt_cache_key(mut self, prompt_cache_key: impl Into<String>) -> Self {
        self.prompt_cache_key = Some(prompt_cache_key.into());
        self
    }

    #[must_use]
    pub fn tool_definitions(mut self, tools: impl IntoIterator<Item = ToolDefinition>) -> Self {
        self.tools = tools.into_iter().collect();
        self
    }

    pub fn build(self) -> Result<Session<F::Service>, SessionBuildError> {
        if self.instructions.trim().is_empty() {
            return Err(SessionBuildError::EmptyInstructions);
        }
        let session_id = self.session_id.unwrap_or_default();
        let prompt_cache_key = self
            .prompt_cache_key
            .unwrap_or_else(|| session_id.to_string());
        if prompt_cache_key.trim().is_empty() {
            return Err(SessionBuildError::EmptyPromptCacheKey);
        }

        let prefix = shared_request_prefix(&prompt_cache_key, self.tools, &self.instructions)
            .map_err(SessionBuildError::SerializePromptPrefix)?;
        let profile = RequestProfile::new(session_id.to_string(), prompt_cache_key, prefix);
        let config = self.config;
        let service = self.factory.make(Arc::new(config.clone()));

        Ok(Session {
            id: session_id,
            client: ResponsesClient::new(service),
            profile,
            state: ManagedSessionState::new(Vec::new()),
            canonical_context: Vec::new(),
            canonical_context_reinjection_pending: false,
            next_call_index: 1,
            next_logical_turn: 1,
            model: config.model,
            thinking: config.thinking,
            service_tier: config.service_tier,
            context_window_tokens: config.context_window_tokens,
            transport_stats: Arc::new(TransportStats::default()),
        })
    }
}

#[derive(Debug)]
pub enum SessionBuildError {
    EmptyInstructions,

    EmptyPromptCacheKey,

    SerializePromptPrefix(serde_json::Error),
}

impl std::fmt::Display for SessionBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInstructions => {
                formatter.write_str("OpenAI session instructions must not be empty")
            }
            Self::EmptyPromptCacheKey => {
                formatter.write_str("OpenAI prompt cache key must not be empty")
            }
            Self::SerializePromptPrefix(value) => write!(
                formatter,
                "failed to fingerprint the immutable prompt prefix: {value}"
            ),
        }
    }
}

impl std::error::Error for SessionBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SerializePromptPrefix(value) => Some(value),
            _ => None,
        }
    }
}

pub struct Session<S> {
    pub(super) id: SessionId,
    pub(super) client: ResponsesClient<S>,
    pub(super) profile: RequestProfile,
    pub(super) state: ManagedSessionState,
    pub(super) canonical_context: Vec<ResponseItem>,
    pub(super) canonical_context_reinjection_pending: bool,
    pub(super) next_call_index: u32,
    next_logical_turn: u64,
    pub(super) model: Model,
    pub(super) thinking: Thinking,
    pub(super) service_tier: ServiceTier,
    pub(super) context_window_tokens: u64,
    pub(super) transport_stats: Arc<TransportStats>,
}

impl<S> Session<S> {
    #[must_use]
    pub const fn id(&self) -> SessionId {
        self.id
    }

    pub const fn turn(&mut self) -> ResponseTurn<'_, S> {
        let logical_turn = self.next_logical_turn;
        self.next_logical_turn = self.next_logical_turn.saturating_add(1);
        ResponseTurn {
            session: self,
            logical_turn,
            completed_generation: false,
        }
    }

    #[must_use]
    pub fn history_len(&self) -> usize {
        self.state.history_len()
    }

    pub fn history(&self) -> impl ExactSizeIterator<Item = &ResponseItem> {
        self.state.history()
    }

    #[must_use]
    pub fn active_context_tokens(&self) -> u64 {
        self.state.active_context_tokens()
    }
}

pub struct ResponseTurn<'session, S> {
    pub(super) session: &'session mut Session<S>,
    pub(super) logical_turn: u64,
    pub(super) completed_generation: bool,
}

impl<S> ResponseTurn<'_, S> {
    #[must_use]
    pub fn active_context_tokens(&self) -> u64 {
        self.session.active_context_tokens()
    }
}

impl<S> ResponseTurn<'_, S>
where
    S: Service<ResponsesAttempt, Response = ResponsesServiceResponse> + MaybeSend,
    S::Error: Into<ResponseError> + MaybeSend,
    S::Future: MaybeSend,
{
    pub fn create(&mut self, input: impl Into<ResponseInput>) -> Response<'_> {
        let (sink, raw_events) = EventSink::channel(self.session.profile.session_id().to_owned());
        drop(raw_events);
        let (response_events, events) = mpsc::channel(RESPONSE_EVENT_CAPACITY);
        let run = Box::pin(run_create(self, input.into(), sink, response_events));
        Response::new(events, run)
    }

    pub async fn compact(&mut self) -> Result<CompletedCompaction, ResponseError> {
        run_compact(self).await
    }
}
