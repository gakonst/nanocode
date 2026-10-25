pub use nanocode_tools::tool;
pub mod upstream;

#[doc(hidden)]
pub mod __private {
    pub use nanocode_tools::__private::*;
}

pub use nanocode_backend_claude_client::*;

mod environment;
pub use environment::ClaudeEnvironment;
mod hooks;
mod instructions;
mod prompt;
pub use hooks::{
    ClaudeHookFuture, ClaudeLifecycleDecision, ClaudeLifecycleEvent, ClaudeLifecycleInvocation,
    ClaudeLifecycleOutcome, ClaudeToolDecision, ClaudeToolHooks,
};

pub use nanocode_agent_core::ClaudeModel;

mod agent;
pub use agent::{
    Claude, ClaudeBuilder, ClaudeCheckpointView, ClaudeNestedToolUpdate, ClaudeToolInvocation,
    ClaudeToolProgress, ClaudeToolReply, ClaudeTools, decode_checkpoint, decode_session_checkpoint,
    default_context_window_tokens, rewind_checkpoint, session_checkpoint, web_search_tool,
};

pub mod durability;

pub mod execution;

#[cfg(not(target_family = "wasm"))]
mod claude_bash;
#[cfg(not(target_family = "wasm"))]
mod claude_files;
mod claude_tools;
pub use claude_tools::install_shared_tools;
mod code_mode_policy;
mod host_tools;
pub use host_tools::{host_file_tools, host_tools};
#[cfg(not(target_family = "wasm"))]
mod defaults;
#[cfg(not(target_family = "wasm"))]
pub use defaults::{
    hand_tools, install_default_tools, install_default_tools_with_executor,
    install_default_tools_with_process_routes, install_default_tools_with_runner_pool,
    runner_tools, runner_tools_with_executor,
};

pub use nanocode_backend_claude_tools::tools;

mod runtime;
pub use runtime::ClaudeHarness;
