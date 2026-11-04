mod agent;
mod error;
mod harness;
mod runtime_tools;
pub mod session;
pub mod usage;
pub use agent::{
    Agent, AgentHandle, AgentSessionContext, BuilderBackend, PromptRequest, PromptRoute,
    SideAnswer, SpawnOptions, SteerReceiptStatus, Turn, TurnControl, TurnResult, WeakAgent,
};
pub use error::{
    BackendError, BackendFailure, CompactionRecovery, ExecutionPolicyDisposition, NanocodeError,
    Result,
};
pub use harness::{
    ClaudeModel, HarnessFamily, ModelCapabilities, ModelInfo, ModelSelection, ModelTransport,
    model_catalog,
};
pub use nanocode_types::{CustomModel, Model, ReasoningMode, Thinking, events::AgentEvents};
pub use session::{
    Capabilities, ForkPoint, ForkRequest, Lineage, Mutability, Origin, Persistence,
    SessionCheckpoint, SessionInfo,
};
pub use usage::{
    AgentUsage, CostStatus, EstimatedUsdCost, ReportedCost, ReportedTurnUsage, ServiceTier,
    TurnUsage, UsageRecord, UsdAmount,
};

pub mod backend {
    pub use crate::agent::backend::*;
    pub use crate::agent::{AgentOwner, adopt, created, created_batch};
    pub use crate::session::TurnBoundary;
}

pub mod events {
    pub use nanocode_types::events::*;
    pub use nanocode_types::responses::AgentMessageContent;
}

pub mod input {
    pub use nanocode_types::{
        ImageDetail, Prompt, PromptInput, PromptMessage, PromptMessageRole, UserInput,
        responses::{AgentMessageContent, ContentItem},
    };
}

pub use nanocode_types::BackendId;
pub use runtime_tools::{AgentToolsFactory, RuntimeTools};

pub trait BackendKind {
    type Model: Clone + Send + Sync + 'static;

    fn backend_id() -> BackendId;

    fn model_id(model: &Self::Model) -> &str;
}

pub trait BackendRegistration: Send + Sync {
    fn id(&self) -> BackendId;

    fn default_model(&self) -> String;

    fn default_thinking(&self) -> Option<Thinking> {
        None
    }

    fn validate_model(&self, model: &str) -> Result<()>;

    fn build(
        &self,
        request: BackendRequest,
    ) -> backend::BackendFuture<Result<(Agent, AgentEvents)>>;
}

impl BackendRequest {
    #[must_use]
    pub fn agent_handle(&self, session_id: impl Into<std::sync::Arc<str>>) -> AgentHandle {
        self.agent_handle_with(session_id, std::sync::Arc::new(backend::CrossHarnessOnly))
    }

    #[must_use]
    pub fn agent_handle_with(
        &self,
        session_id: impl Into<std::sync::Arc<str>>,
        native: std::sync::Arc<dyn backend::AgentFactory>,
    ) -> AgentHandle {
        AgentHandle::new(session_id, self.model.clone(), native)
            .with_spawn_factory(std::sync::Arc::clone(&self.spawn_factory))
    }
}

pub struct BackendRequest {
    pub model: ModelSelection,

    pub thinking: Option<Thinking>,

    pub options: SpawnOptions,

    pub workspace: std::path::PathBuf,

    pub instructions: Option<String>,

    pub spawn_factory: std::sync::Arc<dyn backend::AgentFactory>,

    pub host_context: Option<std::sync::Arc<str>>,

    pub tools: RuntimeTools,

    pub checkpoint: Option<SessionCheckpoint>,
}
