pub use nanocode_code_mode::{code_mode, code_mode_description, code_mode_spec};
pub use nanocode_home::SessionEnvironment;
pub use nanocode_media::image;
pub use nanocode_tools::{
    Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult, tool,
};

pub mod contract {
    pub use nanocode_tools::*;
}

pub mod embedded;
mod function_tool;

pub trait ToolRecovery: Send + Sync + 'static {
    fn recover<'a>(
        &'a self,
        name: &'a str,
        context: nanocode_tools::ToolContext<'a>,
    ) -> embedded::HostFuture<'a, Option<nanocode_tools::ToolOutput>>;
}

pub use function_tool::{Failure, FunctionHandler, FunctionTool, MaybeSend, TypedHandler};
mod pins;
mod policy;
mod selection;
pub use nanocode_code_mode::CodeModeToolObservation;
pub use pins::TextPins;
pub use policy::{CodeModeToolPolicy, WorkspaceToolRouting};
#[cfg(not(target_family = "wasm"))]
pub use selection::{AttachTarget, RemoteRunnerTools};
pub use selection::{ToolExposure, ToolSource, Tools, ToolsBuildError, ToolsBuilder};
#[cfg(not(target_family = "wasm"))]
pub mod attachment;
#[cfg(not(target_family = "wasm"))]
pub mod runtime;
#[cfg(not(target_family = "wasm"))]
pub mod workspace_runtime;
#[cfg(not(target_family = "wasm"))]
pub(crate) use nanocode_tools::ToolOutputContent;
#[cfg(not(target_family = "wasm"))]
pub use workspace_runtime::WorkspaceTools;
mod code_mode_order;

#[cfg(target_family = "wasm")]
pub mod runtime {
    pub use crate::embedded::{
        EmbeddedToolRuntime as ToolRuntime, EmbeddedToolRuntimeControl as ToolRuntimeControl,
        OwnedToolContext,
    };
    pub use crate::{ToolExposure, ToolSource, Tools, ToolsBuildError, ToolsBuilder};
}

#[doc(hidden)]
pub mod host {
    #[cfg(not(target_family = "wasm"))]
    pub use crate::runtime::RoutingCatalog;
    pub use crate::selection::normalize_public_tool_name;
    #[cfg(not(target_family = "wasm"))]
    pub use crate::selection::{PublicToolCatalogError, validate_public_tool_catalog_names};
}

#[doc(hidden)]
pub mod __private {
    pub use nanocode_tools::__private::*;
    pub fn model_contract(
        runtime: &crate::runtime::ToolRuntime,
        session_id: &str,
    ) -> (Vec<crate::ToolDefinition>, Vec<(String, String)>) {
        runtime.model_contract(session_id)
    }
}
