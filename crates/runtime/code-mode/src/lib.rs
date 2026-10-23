#![deny(rustdoc::broken_intra_doc_links)]
#![cfg_attr(target_family = "wasm", allow(clippy::module_name_repetitions))]

pub mod code_mode;

#[path = "code_mode/description.rs"]
pub mod code_mode_description;

pub mod code_mode_order;

#[path = "code_mode/spec.rs"]
pub mod code_mode_spec;

pub mod embedded;
pub mod remote;
mod toolset;

pub use nanocode_media::image;
pub use nanocode_tools::tool;
pub use toolset::{CodeModeToolObservation, CodeModeToolset};

pub mod contract {
    pub use nanocode_tool_contracts::{
        DEFAULT_TOOL_OUTPUT_TOKENS, HistorySource, SharedHistory, Tool, ToolContext,
        ToolDefinition, ToolError, ToolInput, ToolInputError, ToolOutput, ToolOutputBody,
        ToolOutputContent, ToolOutputWire, ToolProcessTraceWire, ToolResult,
    };
}

pub use contract::{
    Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolOutputBody, ToolOutputContent,
    ToolResult,
};

#[doc(hidden)]
pub mod __private {
    pub use nanocode_tools::__private::*;
}
