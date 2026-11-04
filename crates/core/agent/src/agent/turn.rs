use super::backend::{BackendFuture, BackendTurnKey, LifecycleBackend};
use super::*;
use nanocode_types::PromptValidationError;

#[must_use = "a turn continues running when dropped; await result(), control it, or explicitly drop it"]
pub struct Turn {
    #[doc(hidden)]
    pub turn_id: String,
    #[doc(hidden)]
    pub control: TurnControl,
    #[doc(hidden)]
    pub request_id: Option<String>,
    #[doc(hidden)]
    pub events: AgentEvents,
    #[doc(hidden)]
    pub result: BackendFuture<Result<TurnResult>>,
}

pub enum PromptRoute {
    Started(Turn),

    Steered,
}

impl Turn {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.turn_id
    }

    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    #[must_use]
    pub fn control(&self) -> TurnControl {
        self.control.clone()
    }

    pub async fn steer(&self, prompt: impl Into<Prompt>) -> Result<()> {
        self.control.steer(prompt).await
    }

    pub async fn steer_with_id(&self, id: String, prompt: impl Into<Prompt>) -> Result<()> {
        self.control.steer_with_id(id, prompt).await
    }

    pub async fn withdraw_steer(&self, id: String) -> Result<bool> {
        self.control.withdraw_steer(id).await
    }

    pub async fn cancel(&self) -> Result<()> {
        self.control.cancel().await
    }

    pub fn try_next(&mut self) -> Option<AgentEvent> {
        self.events.try_recv()
    }

    pub async fn result(self) -> Result<TurnResult> {
        let Self { events, result, .. } = self;
        drop(events);
        result.await
    }
}

impl Stream for Turn {
    type Item = AgentEvent;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.events).poll_next(context)
    }
}

impl Future for Turn {
    type Output = Result<TurnResult>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.result.as_mut().poll(context)
    }
}

#[derive(Clone)]
pub struct TurnControl {
    #[doc(hidden)]
    pub key: BackendTurnKey,
    #[doc(hidden)]
    pub backend: Arc<dyn LifecycleBackend>,
}

impl TurnControl {
    pub async fn steer(&self, prompt: impl Into<Prompt>) -> Result<()> {
        let prompt = prompt.into();
        prompt.validate().map_err(steer_validation_error)?;
        self.backend.steer(self.key, prompt).await
    }

    pub async fn steer_with_id(&self, id: String, prompt: impl Into<Prompt>) -> Result<()> {
        if id.is_empty() {
            return Err(NanocodeError::InvalidRequest(
                "steer identity must not be empty".into(),
            ));
        }
        let prompt = prompt.into();
        prompt.validate().map_err(steer_validation_error)?;
        self.backend.steer_with_id(self.key, id, prompt).await
    }

    pub async fn withdraw_steer(&self, id: String) -> Result<bool> {
        self.backend.withdraw_steer(self.key, id).await
    }

    pub async fn cancel(&self) -> Result<()> {
        self.backend.cancel(self.key).await
    }
}

fn steer_validation_error(error: PromptValidationError) -> NanocodeError {
    let message = match error {
        PromptValidationError::EmptyInstruction => "steer instruction must not be empty".to_owned(),
        error => error.to_string(),
    };
    NanocodeError::InvalidRequest(message)
}

#[derive(Clone)]
pub struct TurnResult {
    #[doc(hidden)]
    pub request_id: Option<String>,
    #[doc(hidden)]
    pub turn_id: Option<String>,
    #[doc(hidden)]
    pub final_message: String,
    #[doc(hidden)]
    pub usage: Option<TurnUsage>,
    #[doc(hidden)]
    pub boundary: Option<TurnBoundary>,
}

impl TurnResult {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    #[must_use]
    pub fn final_message(&self) -> &str {
        &self.final_message
    }

    #[must_use]
    pub fn into_final_message(self) -> String {
        self.final_message
    }

    #[must_use]
    pub const fn usage(&self) -> Option<&TurnUsage> {
        self.usage.as_ref()
    }

    #[must_use]
    pub fn usage_record(&self) -> Option<crate::UsageRecord> {
        let usage = self.usage.as_ref()?;
        Some(crate::UsageRecord::new(
            self.checkpoint()?.model().model,
            usage,
        ))
    }

    #[must_use]
    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    #[must_use]
    pub fn checkpoint(&self) -> Option<SessionCheckpoint> {
        self.try_checkpoint().ok().flatten()
    }

    pub fn try_checkpoint(&self) -> Result<Option<SessionCheckpoint>> {
        let Some(boundary) = self.boundary.as_ref() else {
            return Ok(None);
        };
        let checkpoint = boundary.checkpoint()?;
        Ok(Some(if checkpoint.turn_id().is_none() {
            checkpoint.with_turn_id(self.turn_id.clone())
        } else {
            checkpoint
        }))
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn boundary(&self) -> Option<&TurnBoundary> {
        self.boundary.as_ref()
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn from_backend(
        request_id: Option<String>,
        final_message: String,
        usage: Option<TurnUsage>,
        boundary: Option<TurnBoundary>,
    ) -> Self {
        Self {
            request_id,
            turn_id: None,
            final_message,
            usage,
            boundary,
        }
    }

    #[doc(hidden)]
    pub fn with_turn_id(mut self, turn_id: String) -> Self {
        if self.turn_id.is_none() {
            self.turn_id = Some(turn_id);
        }
        self
    }
}

#[derive(Clone, Debug)]
pub struct SideAnswer {
    text: String,
    usage: Option<TurnUsage>,
}

impl SideAnswer {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(text: String, usage: Option<TurnUsage>) -> Self {
        Self { text, usage }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn into_text(self) -> String {
        self.text
    }

    #[must_use]
    pub const fn usage(&self) -> Option<&TurnUsage> {
        self.usage.as_ref()
    }
}

impl fmt::Debug for TurnResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TurnResult")
            .field("final_message", &self.final_message)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct PromptRequest {
    #[doc(hidden)]
    pub prompt: Prompt,
    #[doc(hidden)]
    pub request_id: Option<String>,
    #[doc(hidden)]
    pub cancel_on_admission: bool,
}

impl PromptRequest {
    #[must_use]
    pub fn new(prompt: impl Into<Prompt>) -> Self {
        Self {
            prompt: prompt.into(),
            request_id: None,
            cancel_on_admission: false,
        }
    }

    #[must_use]
    pub fn request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    #[must_use]
    pub const fn cancel_on_admission(mut self) -> Self {
        self.cancel_on_admission = true;
        self
    }
}

impl From<Prompt> for PromptRequest {
    fn from(prompt: Prompt) -> Self {
        Self::new(prompt)
    }
}

impl From<String> for PromptRequest {
    fn from(prompt: String) -> Self {
        Self::new(prompt)
    }
}

impl From<&str> for PromptRequest {
    fn from(prompt: &str) -> Self {
        Self::new(prompt)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SpawnOptions {
    #[doc(hidden)]
    pub model: Option<Model>,
    #[doc(hidden)]
    pub harness: Option<crate::HarnessFamily>,
    #[doc(hidden)]
    pub harness_model: Option<crate::ModelSelection>,
    #[doc(hidden)]
    pub thinking: Option<Thinking>,
    pub reasoning_mode: Option<crate::ReasoningMode>,
    #[doc(hidden)]
    pub stateless_http: bool,
}

impl SpawnOptions {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            model: None,
            harness: None,
            harness_model: None,
            thinking: None,
            reasoning_mode: None,
            stateless_http: false,
        }
    }

    #[must_use]
    pub fn model(mut self, model: Model) -> Self {
        self.model = Some(model);
        self.harness_model = Some(crate::ModelSelection::Codex(model));
        self
    }

    #[must_use]
    pub fn harness(mut self, family: crate::HarnessFamily) -> Self {
        self.harness = Some(family);
        self
    }

    #[must_use]
    pub fn harness_model(mut self, model: crate::ModelSelection) -> Self {
        self.model = model.as_codex();
        self.harness_model = Some(model);
        self
    }

    #[must_use]
    pub fn selected_harness(&self) -> Option<crate::HarnessFamily> {
        self.harness.clone()
    }

    #[must_use]
    pub fn selected_harness_model(&self) -> Option<crate::ModelSelection> {
        self.harness_model.clone()
    }

    #[must_use]
    pub fn selected_family(&self) -> Option<crate::HarnessFamily> {
        match (self.harness.clone(), self.harness_model.clone()) {
            (Some(family), _) => Some(family),
            (None, Some(model)) => Some(model.family()),
            (None, None) => None,
        }
    }

    pub fn validate_harness(&self) -> Result<()> {
        if self
            .harness
            .as_ref()
            .is_some_and(|family| family.as_str().trim().is_empty())
        {
            return Err(NanocodeError::InvalidRequest(
                "backend identity must not be empty".into(),
            ));
        }
        if let Some(model) = &self.harness_model {
            model.validate()?;
        }
        if let (Some(family), Some(model)) = (self.harness.clone(), self.harness_model.clone())
            && family != model.family()
        {
            return Err(NanocodeError::InvalidRequest(
                "model does not belong to selected harness".into(),
            ));
        }
        if let (Some(model), Some(thinking)) = (self.harness_model.as_ref(), self.thinking) {
            model
                .capabilities(crate::ModelTransport::Native)
                .check_thinking(thinking)?;
        }
        Ok(())
    }

    pub fn resolve(self, parent: crate::ModelSelection, parent_thinking: Thinking) -> Result<Self> {
        self.validate_harness()?;
        let family = self.selected_family().unwrap_or(parent.family());
        let model = self.harness_model.clone().unwrap_or_else(|| {
            if family == parent.family() {
                parent.clone()
            } else {
                family.default_model()
            }
        });
        if model.family() != family {
            return Err(NanocodeError::InvalidRequest(
                "model does not belong to selected harness".into(),
            ));
        }
        let thinking = self.thinking.unwrap_or_else(|| {
            if model == parent {
                parent_thinking
            } else {
                model.default_thinking()
            }
        });
        let resolved = self.harness(family).harness_model(model).thinking(thinking);
        resolved.validate_harness()?;
        Ok(resolved)
    }

    #[must_use]
    pub const fn thinking(mut self, thinking: Thinking) -> Self {
        self.thinking = Some(thinking);
        self
    }

    #[must_use]
    pub const fn reasoning_mode(mut self, reasoning_mode: crate::ReasoningMode) -> Self {
        self.reasoning_mode = Some(reasoning_mode);
        self
    }

    #[must_use]
    pub const fn selected_reasoning_mode(&self) -> Option<crate::ReasoningMode> {
        self.reasoning_mode
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn stateless_http(mut self) -> Self {
        self.stateless_http = true;
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn selected_model(&self) -> Option<Model> {
        self.model
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn selected_thinking(&self) -> Option<Thinking> {
        self.thinking
    }
}
