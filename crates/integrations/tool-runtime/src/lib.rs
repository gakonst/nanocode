pub use nanocode_tool_host::*;

#[cfg(not(target_family = "wasm"))]
pub mod attachment;
#[cfg(not(target_family = "wasm"))]
pub mod mcp;
#[cfg(target_family = "wasm")]
pub use nanocode_mcp_tools as mcp;
#[cfg(not(target_family = "wasm"))]
mod prepared;
pub mod remote;
#[cfg(not(target_family = "wasm"))]
pub use nanocode_process as shell;
#[cfg(not(target_family = "wasm"))]
pub(crate) use nanocode_tool_host::runtime::DynamicToolProvider;
#[cfg(not(target_family = "wasm"))]
pub use shell::{ForegroundOutput, ambient_sensitive_environment, run_bash};
