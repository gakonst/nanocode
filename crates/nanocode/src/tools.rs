pub use nanocode_backend_codex::tools::*;

#[cfg(target_family = "wasm")]
pub use nanocode_tool_runtime::mcp;
#[cfg(not(target_family = "wasm"))]
pub use nanocode_tool_runtime::{
    ForegroundOutput, ambient_sensitive_environment, mcp, remote, run_bash, shell,
};

#[cfg(not(target_family = "wasm"))]
pub mod attachment {
    pub use nanocode_tool_runtime::attachment::*;

    use nanocode_tool_runtime::Tools;

    pub fn run_runner(
        config: RunnerConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AttachmentError>> + Send>>
    {
        Box::pin(async move {
            let tools = Tools::builder()
                .without_defaults()
                .build()
                .map_err(|error| AttachmentError::Catalog(error.to_string().into()))?;
            run_runner_with_tools(config, tools).await
        })
    }

    pub fn run_runner_with_tools(
        config: RunnerConfig,
        tools: Tools,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AttachmentError>> + Send>>
    {
        nanocode_agent_host::run_runner_with_workspace_tools(config, tools, |workspace| {
            nanocode_backend_codex::tools::WorkspaceTools::new(workspace).into_source()
        })
    }
}
