mod capacity;
mod diagnostics;
mod durable;
pub use durable::{
    MemorySubagentStore, RestoreReport, SubagentStore, SubagentStoreFuture, checkpoint_key,
};

pub use diagnostics::{CompletionError, CompletionErrorCode};
mod authority;
mod harness;
mod message;
mod model;
mod platform;
mod policy;
mod roster;
mod routing;
mod runtime;
pub use authority::{AuthorityError, RootAgentAuthority};
pub use policy::{SpawnPolicy, SpawnPolicyError};
pub use roster::{SubagentNode, SubagentRoster};
pub use routing::{SpawnDecision, SpawnRoute, SpawnRouter};
mod task_tree;
mod tools;

pub use model::{
    AgentContext, AgentDescriptor, AgentId, AgentMessage, AgentMessageUpdate, AgentStatus,
    AgentThread, AgentUpdate, MessageDeliveryState, MessageDisposition, MessageId, MessagePriority,
    MessagePurpose, MessageSender, ScopedAgentUpdate, SubagentRuntimeId, ThreadId,
};
pub use runtime::{
    AgentDirectoryEntry, AgentSummary, COMPLETION_INSTRUCTIONS_LEAD, CompletionPrompt,
    CompletionStatus, MessageReceipt, ParentResolver, Registry, Restoration, RunningSubagents,
    SessionReleased, SubagentActivity, SubagentCompletion, SubagentControl, channel,
    completion_continuation_prompt, parse_completion_prompt,
};
pub use tools::{
    AgentStartReport, AgentTask, AgentToolResult, install_tools, start_agent, start_agent_with,
    start_agents, start_agents_observed, start_fork_agent,
};

pub const DEFAULT_MAX_SUBAGENTS: usize = usize::MAX;

pub const COMPLETION_REQUEST_PREFIX: &str = "subagent-completion:";

pub const DEFAULT_MAX_RESIDENT_SUBAGENTS: usize = 16;

pub const DURABLE_MAX_RESIDENT_SUBAGENTS: usize = 2;

pub use tools::install_claude_tools;
