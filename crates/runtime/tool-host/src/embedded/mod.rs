pub use nanocode_code_mode::embedded::*;
pub(crate) mod runtime;
pub use runtime::{EmbeddedToolRuntime, EmbeddedToolRuntimeControl};

pub fn bind_host(tools: crate::Tools, host: impl CodeModeHost) -> crate::Tools {
    tools.code_mode_host(host)
}
