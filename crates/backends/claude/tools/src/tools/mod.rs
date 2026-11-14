pub mod bash;
pub mod host;
pub mod input;
pub mod tasks;
pub mod text_files;
pub mod web;
#[cfg(not(target_family = "wasm"))]
pub mod workspace_files;

pub use bash::{
    BashExecution, BashExecutor, BashRequest, BashResult, ClaudeBash, SandboxBashExecutor,
};
pub use host::{
    ClaudeHost, ClaudeHostTools, ClaudeMcp, ClaudeMcpProvider, HostContext, HostRequest, HostTool,
    ImageSource, McpToolDefinition, ToolContent, ToolOutput, ToolResultBlock,
};
pub use nanocode_home::SessionEnvironment;
pub use tasks::ClaudeTasks;
pub use web::{
    ApprovedPage, ApprovedWebFetchSource, ApprovedWebProvider, ClaudeWeb, WebFetchRequest,
    WebSearchRequest,
};
#[cfg(not(target_family = "wasm"))]
pub use workspace_files::ClaudeWorkspaceFiles;

#[cfg(not(target_family = "wasm"))]
pub mod media;
#[cfg(not(target_family = "wasm"))]
pub use media::MediaReadOptions;

pub use nanocode_tools::tool;

pub mod contract {
    pub use nanocode_tools::*;
}

#[doc(hidden)]
pub use nanocode_tools::__private;
