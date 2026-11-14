use std::sync::Arc;

use std::{collections::VecDeque, path::PathBuf};

use crate::client::{
    __private::{EventSink, ModelConfig, ResponsesServiceFactory, into_openai_parts},
    OpenAi, ReasoningMode, ResponseError,
    auth::OpenAiAuthMode,
    session::SessionId,
    tower::{ResponsesAttempt, ResponsesClient, ResponsesServiceResponse, StandardServiceFactory},
    transport::{ResponsesHistory, ResponsesTransport, TransportStats},
};
use crate::client::{Model, Prompt, Thinking, events::AgentEvents, pricing::ServiceTier};

use nanocode_tool_host::Tools;
use nanocode_tool_host::image::ImageHistory;

use nanocode_tool_host::ToolsBuildError;

use nanocode_sync::oneshot;

use nanocode_sync::{mpsc, watch};

use crate::client::tower::Service;

use nanocode_trace::{Instrument, info, info_span};

use crate::prompt_cache::{ModelPromptCache, SharedPromptCache};

use crate::session::SessionSnapshot;
use crate::{
    NanocodeError, Result,
    session::{
        Capabilities, ForkRequest, Lineage, Persistence, SessionCheckpoint, SessionInfo,
        TurnBoundary,
    },
    usage::TurnUsage,
};

use crate::{
    model::run::{
        CompletedModelTurn, HistoryCheckpoint, ModelCheckpoint, ModelCompactOutcome, ModelRun,
        ModelTurnOutcome, PreparedCheckpoint, TurnSteering, prepare_checkpoint,
        prepare_history_checkpoint, prepare_resumed_checkpoint,
    },
    session::{CommittedSession, SessionResume},
};

const COMMAND_CAPACITY: usize = 8;

type ToolsFactory =
    Arc<dyn Fn(AgentHandle) -> std::result::Result<Tools, ToolsBuildError> + Send + Sync>;

type SpawnObserver = dyn Fn(&str) + Send + Sync;

enum InitialResume {
    Exact(Box<ModelCheckpoint>),
    History(Box<HistoryCheckpoint>),
}

impl InitialResume {
    fn from_resume(resume: SessionResume) -> Self {
        let SessionResume {
            lineage_id,
            prompt_cache_key,
            workspace,
            canonical_context,
            history,
            client_authored,
            context_baseline,
            reasoning,
            checkpoint,
            ..
        } = resume;
        checkpoint.map_or_else(
            || {
                Self::History(Box::new(HistoryCheckpoint {
                    workspace,
                    provider_session_id: lineage_id,
                    canonical_context,
                    history,
                    client_authored,
                    prompt_cache_key,
                    context_baseline,
                    reasoning,
                }))
            },
            |checkpoint| Self::Exact(Box::new(checkpoint)),
        )
    }

    fn workspace(&self) -> &str {
        match self {
            Self::Exact(checkpoint) => checkpoint.workspace(),
            Self::History(resume) => &resume.workspace,
        }
    }

    fn history_len(&self) -> usize {
        match self {
            Self::Exact(checkpoint) => checkpoint.history().len(),
            Self::History(resume) => resume.history.len(),
        }
    }
}

#[derive(Clone)]

enum ToolsConfiguration {
    Shared(Tools),
    PerAgent(ToolsFactory),
}

impl ToolsConfiguration {
    fn materialize(&self, agent_handle: AgentHandle) -> Result<Tools> {
        let tools = match self {
            Self::Shared(tools) => Ok(tools.clone()),
            Self::PerAgent(factory) => factory(agent_handle).map_err(NanocodeError::from),
        }?;
        crate::builtin_tools::validate_tool_selection(tools).map_err(Into::into)
    }
}

pub mod backend;

mod builder;

pub mod checkpoint;

mod context_source;

mod driver;

pub mod execution;

mod executor;
mod handle;

mod spawn;
mod turn;

pub use backend::BuilderBackend;

pub use builder::CodexBuilder;

pub use context_source::ExecutionEnvironment;
pub use nanocode_agent_core::AgentHandle;
pub use nanocode_agent_core::AgentSessionContext;
#[doc(hidden)]
pub use nanocode_agent_core::backend::{AgentOwner, adopt};
pub use nanocode_agent_core::{Agent, WeakAgent};
pub use nanocode_agent_core::{
    PromptRequest, PromptRoute, SpawnOptions, Turn, TurnControl, TurnResult,
};

use backend::{BackendRuntime, LocalLifecycle};

use builder::{CodexCompatibility, PromptCacheConfig, PromptCacheKey};

use checkpoint::{CheckpointSource, ChildState, is_stateless_http};

pub(crate) use context_source::ContextSource;

use context_source::ContextSourceConfig;

use driver::{AgentDriver, AgentOrigin, BranchSpawner, DriverShutdown};

use execution::{Execution, ExecutionConfig};

pub(crate) use execution::{ExecutionStep, ExecutionSteps};

pub(crate) use executor::{AgentFactory, AgentSend, ResponsesClientBox};

use executor::{ServiceFactory, spawn_driver};

use handle::request_command;

use spawn::{build_agent, spawn_agent_driver, validate};

use turn::{Command, ExecutionOperation, ForkFrom, PromptRouteKind, QueuedTurn, TurnKey};
