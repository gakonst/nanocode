#[cfg(not(target_family = "wasm"))]
use crate::embedded::{CodeModeExecution, CodeModeHostError, CodeModeObserver, HostFuture};
#[cfg(not(target_family = "wasm"))]
use crate::{Tool, ToolContext, ToolDefinition};
pub use nanocode_code_mode::remote::*;
#[cfg(not(target_family = "wasm"))]
mod hand;
#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub use hand::{Hand, HandEndpoint, HandEvent};
#[cfg(not(target_family = "wasm"))]
pub(crate) use native::SocketShutdown;
#[cfg(not(target_family = "wasm"))]
pub use native::{
    BusyItem, BusyKind, ExecutorOptions, HandActivity, SocketPolicy, SocketTransport, serve_accept,
    serve_connect, serve_connect_with, serve_stream,
};
