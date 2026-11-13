#![deny(rustdoc::broken_intra_doc_links)]

pub use nanocode_agent::Agent;
pub use nanocode_agent::input::PromptMessage;
pub use nanocode_agent::{
    AgentEvents, AgentSessionContext, Capabilities, ClaudeModel, CostStatus, EstimatedUsdCost,
    ExecutionPolicyDisposition, ForkPoint, ForkRequest, HarnessFamily, Lineage, ModelCapabilities,
    ModelTransport, Mutability, NanocodeError, Origin, Persistence, PromptRequest, PromptRoute,
    ReportedTurnUsage, ServiceTier, SessionCheckpoint, SessionInfo, SideAnswer, SteerReceiptStatus,
    Turn, TurnControl, TurnResult, TurnUsage, UsageRecord, UsdAmount, WeakAgent,
};
pub use nanocode_agent::{
    BackendId, BackendKind, BackendRegistration as Backend, BackendRequest, ModelSelection,
};
pub use nanocode_agent::{ModelInfo, model_catalog};
pub use nanocode_platform as platform;
pub use nanocode_process_contracts as process;

pub use nanocode_agent::HarnessRequest;
pub use nanocode_backend_claude::ClaudeHarness;
pub use nanocode_backend_codex::CodexHarness;
pub type Harness = nanocode_agent::Harness<Defaults>;
pub type HarnessBuilder = nanocode_agent::HarnessBuilder<Defaults>;
#[cfg(not(target_family = "wasm"))]
pub use nanocode_agent_host as runners;
pub use nanocode_backend_codex::CodexBuilder;

pub use nanocode_agent::{CustomModel, Model, ReasoningMode, Thinking};
pub use nanocode_backend_claude::{Claude, ClaudeClient};
#[cfg(not(target_family = "wasm"))]
pub use runners::{
    AttachmentError, Runner, RunnerEvent, RunnerHandle, RunnerInfo, RunnerPool, RunnerProtocol,
    RunnerServer, RunnerServerConfig, RunnerSpec, RunnerStatus,
};

pub use nanocode_backend_codex::{Codex, client::OpenAi};
pub use nanocode_code_mode::Tool;
pub use nanocode_code_mode::image::{DEFAULT_INLINE_IMAGES, ImageHistory, SharedBlobStore};

pub use nanocode_code_mode::embedded::{
    CodeModeExecution, CodeModeHost, CodeModeHostError, CodeModeObserver, CodeModeUpdate,
    HostFuture,
};
pub use nanocode_code_mode::remote::{
    CodeModeExecutors, ExecutorSelection, HandAuthorizer, HandCallDecision, HandCapability,
    HandDescription, HandEnvironment, HandHostInfo, HandList, HandListener, HandPermission,
    HandProgram, HandRequestDecision, HandTailscale, HandTailscalePeer, HandToolSelection,
    HostInfoLimits, HostInfoSource, RemoteCatalog, RemoteClient, RemoteCodeModeHost,
    RemoteContext, RemoteDelivery, RemoteFrame, RemoteLink, RemoteOperation, RemoteTransport,
    ResumedReply,
};
pub use nanocode_code_mode::tool;
pub use nanocode_tool_runtime::{
    Failure, FunctionHandler, FunctionTool, ToolRecovery, Tools, TypedHandler,
};

pub use nanocode_durability::DurableAgentExt;

#[cfg(not(target_family = "wasm"))]
pub mod hand {
    use std::{net::SocketAddr, path::PathBuf};

    pub use crate::claude::HandFiles;
    pub use nanocode_tool_runtime::remote::{
        Hand, HandCapabilities, HandEndpoint, HandEvent, HandPricing, PricedOperation,
        SocketPolicy, local_host_info,
    };

    pub fn tools(directory: impl Into<PathBuf>) -> Result<crate::Tools, crate::NanocodeError> {
        tools_with(directory, HandFiles::Machine)
    }

    pub fn tools_with(
        directory: impl Into<PathBuf>,
        files: HandFiles,
    ) -> Result<crate::Tools, crate::NanocodeError> {
        let directory = directory.into();
        crate::claude::hand_tools_with(directory.clone(), files)?
            .into_builder()
            .add(crate::tools::WorkspaceTools::new(directory))
            .build()
            .map_err(|error| crate::NanocodeError::InvalidRequest(error.to_string()))
    }

    pub fn connect(
        url: impl Into<String>,
        token: impl Into<String>,
        directory: impl Into<PathBuf>,
    ) -> Result<Hand, crate::NanocodeError> {
        connect_with(url, token, directory, HandFiles::Machine)
    }

    pub fn connect_with(
        url: impl Into<String>,
        token: impl Into<String>,
        directory: impl Into<PathBuf>,
        files: HandFiles,
    ) -> Result<Hand, crate::NanocodeError> {
        let directory = directory.into();
        Ok(Hand::connect(url, token, tools_with(directory.clone(), files)?).directory(directory))
    }

    pub fn listen(
        address: SocketAddr,
        token: impl Into<String>,
        directory: impl Into<PathBuf>,
    ) -> Result<Hand, crate::NanocodeError> {
        listen_with(address, token, directory, HandFiles::Machine)
    }

    pub fn listen_with(
        address: SocketAddr,
        token: impl Into<String>,
        directory: impl Into<PathBuf>,
        files: HandFiles,
    ) -> Result<Hand, crate::NanocodeError> {
        let directory = directory.into();
        Ok(
            Hand::listen(address, token, tools_with(directory.clone(), files)?)
                .directory(directory),
        )
    }
}

pub fn session_store<S: durability::StateStore + 'static>(
    store: S,
) -> durability::Result<durability::SessionStore> {
    nanocode_agent::session_store::<Defaults, S>(store)
}

pub mod turnlog {
    pub use nanocode_agent::{OneLog, OneLogJournals, OneLogStore, Policy};
    pub use nanocode_tool_runtime::effects::{EffectCall, EffectGate};
    pub use nanocode_turnlog::*;

    pub struct Parts {
        pub gate: std::sync::Arc<dyn EffectGate>,
        pub journals: nanocode_code_mode::code_mode::CellJournals,
        pub log: std::sync::Arc<OneLog>,
        pub store: crate::durability::SessionStore,
    }
}

pub async fn turnlog(
    host: std::sync::Arc<dyn turnlog::LogHost>,
    scope: &str,
    policy: turnlog::Policy,
) -> durability::Result<turnlog::Parts> {
    let log = turnlog::OneLog::open(host, scope, policy).await?;
    let store = session_store(log.store())?.with_logged_prompts();
    let journals = log.journals().await?;
    Ok(turnlog::Parts {
        gate: log.gate(),
        journals,
        log,
        store,
    })
}
pub use nanocode_subagents::{
    AgentContext, AgentDescriptor, AgentId, AgentMessageUpdate, AgentStatus, AgentUpdate,
    RunningSubagents, ScopedAgentUpdate, SubagentCompletion, SubagentControl, SubagentRuntimeId,
};
pub type Defaults = nanocode_agent_host::Defaults<CodexHarness, ClaudeHarness>;
pub type Nanocode = nanocode_agent::Runtime<Defaults>;
pub type NanocodeBuilder<B = nanocode_agent::Unselected> =
    nanocode_agent::RuntimeBuilder<B, Defaults>;
pub use nanocode_agent::{
    DurableOutcome, EventSink, PromptRejection, SessionEvent, SessionEvents, SpawnBuilder,
    SubagentUpdates, Unselected, admit_prompt,
};

pub mod session_host {
    pub use nanocode_agent::{
        Attach, Base, Delivery, Driven, EventLog, Halt, ImportedTurn, InboxError, InboxHost,
        LogEntry, LogPage, LoggedEvent, Missing, MovedQueued, Next, OneLog, PassEnd, Recording,
        Replaced, ResumeError, SessionLog, Settled, Started, Submission, Submitted, TurnInbox,
        TurnLog, TurnRecord, TurnRetry, TurnState, Unresumable, Verdict, Wake, base_checkpoint,
        settle_pending, submission_digest,
    };
}

pub mod agent {

    pub use crate::durability;
    pub use nanocode_agent::Agent;
    pub use nanocode_agent::{
        AgentEvents, AgentSessionContext, BuilderBackend, CostStatus, EstimatedUsdCost,
        ExecutionPolicyDisposition, NanocodeError, PromptRequest, PromptRoute, ReportedTurnUsage,
        Result, ServiceTier, SpawnOptions, Turn, TurnControl, TurnResult, TurnUsage, UsdAmount,
        WeakAgent, events, input, session, usage,
    };
    pub use nanocode_agent::{
        AgentHandle, Capabilities, ForkPoint, ForkRequest, Lineage, Mutability, Origin,
        Persistence, SessionCheckpoint, SessionInfo,
    };

    pub use nanocode_backend_codex::{
        CodexBuilder, ExecutionEnvironment, durability::DurableSessionExt, execution,
    };
}

#[doc(inline)]
pub use nanocode_backend_claude as claude;

#[doc(inline)]
pub use nanocode_durability as durability;

#[doc(inline)]
pub use nanocode_subagents as subagents;

#[doc(inline)]
pub use nanocode_backend_codex::client as oai;

pub use nanocode_backend_codex::client as codex;

#[doc(inline)]
pub use nanocode_backend_codex::client::transport::layer as http;

pub mod tools;

#[doc(inline)]
pub use tools as oai_tools;

#[doc(inline)]
pub use nanocode_backend_claude::tools as claude_tools;

#[cfg(all(not(target_family = "wasm"), feature = "cua"))]
pub use nanocode_computer as cua;
#[cfg(all(not(target_family = "wasm"), feature = "cua"))]
pub use nanocode_computer::{ComputerConfig, ComputerTools};

pub mod prelude {
    pub use crate::ClaudeHarness;
    pub use crate::CodexHarness;

    pub use crate::DurableAgentExt;
    pub use crate::Nanocode;
    pub use crate::SubagentUpdates;
    pub use crate::Tool;
    pub use crate::{AgentStatus, AgentUpdate, ScopedAgentUpdate, SubagentControl};
    #[cfg(not(target_family = "wasm"))]
    pub use crate::{Runner, RunnerPool, RunnerServerConfig};

    pub use crate::Tools;
    pub use crate::tool;
    pub use crate::{Agent, ClaudeModel, HarnessFamily, Model, ModelSelection, Thinking};
    pub use crate::{Claude, ClaudeClient};

    pub use crate::durability::{MemoryStore, SessionStore};
    pub use crate::{AgentEvents, CodexBuilder, OpenAi};
    #[cfg(all(not(target_family = "wasm"), feature = "cua"))]
    pub use crate::{ComputerConfig, ComputerTools};
}

#[doc(hidden)]
pub mod __private {
    pub use nanocode_tool_runtime::__private::*;
}

pub mod codex_tools {
    #[cfg(not(target_family = "wasm"))]
    pub use crate::tools::attachment;
    pub use nanocode_backend_codex::builtin_tools::*;
}

pub use tools as code_mode;

pub use nanocode_tools as tool_contract;

pub mod backend {
    pub use nanocode_agent::backend::*;
    pub use nanocode_agent::input::{Prompt, PromptInput};
    pub use nanocode_agent::{
        Agent, AgentHandle, AgentSessionContext, AgentToolsFactory, BackendId, BackendKind,
        BackendRegistration, BackendRequest, Capabilities, ForkRequest, HarnessFamily,
        ModelSelection, NanocodeError, Result, RuntimeTools, SessionCheckpoint, SpawnOptions,
        Thinking, TurnResult,
    };
    pub use nanocode_durability::{CheckpointFormat, DurableBackendRegistration};
    pub use nanocode_tools::{Tool, ToolCollection, ToolContext, ToolInput};
}

#[cfg(not(target_family = "wasm"))]
pub use nanocode_store_jsonl::{CompactionStats, JsonlArchiver, JsonlStore};
