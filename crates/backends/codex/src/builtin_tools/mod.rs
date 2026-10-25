pub use crate::client::ImageDetail;
pub use nanocode_tools::{
    Tool, ToolContext, ToolInput, ToolOutput, ToolOutputBody, ToolOutputContent, ToolResult,
};

pub mod contract {
    pub use nanocode_tools::*;
}
pub use nanocode_media::image;

pub mod apply_patch;
mod code_mode_policy;
pub mod extensions;
pub mod standard;
pub use standard::StandardTool;
#[cfg(not(target_family = "wasm"))]
mod gateway_unsupported;
mod host;
pub use host::{host_file_tools, host_tools};
mod plan;
mod shell;
#[cfg(not(target_family = "wasm"))]
mod view_image;
#[cfg(not(target_family = "wasm"))]
pub mod workspace_runtime;
#[cfg(not(target_family = "wasm"))]
pub use workspace_runtime::WorkspaceTools;

mod runtime_config;

pub use runtime_config::{ImageGenerationConfig, WebSearchConfig};
#[cfg(not(target_family = "wasm"))]
pub mod image_generation;
#[cfg(not(target_family = "wasm"))]
pub mod web_search;
#[cfg(target_family = "wasm")]
#[path = "web_search/host.rs"]
mod web_search_host;
#[cfg(target_family = "wasm")]
pub use web_search_host::web_search_tool;

pub use nanocode_tools::tool;

#[doc(hidden)]
pub use nanocode_tools::__private;

pub fn tool_runtime(
    workspace: impl AsRef<std::path::Path>,
    config: &crate::client::tower::ResponsesServiceConfig,
    tools: &nanocode_tool_host::Tools,
) -> nanocode_tool_host::runtime::ToolRuntime {
    let workspace = workspace.as_ref();
    let tools = tools
        .clone()
        .with_code_mode_policy(code_mode_policy::standard_tool_policy());
    if tools.has_code_mode_host() {
        return nanocode_tool_host::runtime::ToolRuntime::new_with_tools(
            workspace,
            Vec::new(),
            &tools,
        );
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let tools = if tools.workspace_enabled() && tools.workspace_source().is_none() {
            tools.with_workspace_source(WorkspaceTools::new(workspace).source)
        } else {
            tools
        };
        let mut handlers = if tools.web_search_enabled() || tools.image_generation_enabled() {
            nanocode_tls::install_default_rustls_crypto_provider();
            handlers(
                tools.web_search_enabled().then(|| WebSearchConfig {
                    endpoint: config.search_endpoint(),
                    auth: config.auth.clone(),
                    http_layer: config.http_layer.clone(),
                }),
                tools
                    .image_generation_enabled()
                    .then(|| ImageGenerationConfig {
                        api_base_url: config.api_base_url.clone(),
                        auth: config.auth.clone(),
                        save_root: workspace.to_path_buf(),
                        http_layer: config.http_layer.clone(),
                    }),
                tools.remote_http_client().unwrap_or_default(),
            )
        } else {
            Vec::new()
        };
        if let Some(gateway) = config.responses_gateway.as_deref() {
            handlers = gateway_unsupported::wrap(handlers, gateway.name());
        }
        if tools.workspace_enabled() {
            handlers.push(std::sync::Arc::new(plan::UpdatePlanTool::new()));
        }
        nanocode_tool_host::runtime::ToolRuntime::new_with_tools(workspace, handlers, &tools)
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = config;
        nanocode_tool_host::runtime::ToolRuntime::new_with_tools(workspace, Vec::new(), &tools)
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn handlers(
    web: Option<WebSearchConfig>,
    image: Option<ImageGenerationConfig>,
    client: reqwest::Client,
) -> Vec<std::sync::Arc<dyn Tool>> {
    let mut tools: Vec<std::sync::Arc<dyn Tool>> = Vec::new();
    if let Some(config) = web {
        tools.push(std::sync::Arc::new(
            web_search::WebSearchHandler::with_client(config, client.clone()),
        ));
    }
    if let Some(config) = image {
        tools.push(std::sync::Arc::new(
            image_generation::ImageGenerationHandler::with_client(config, client),
        ));
    }
    tools
}

pub(crate) fn validate_tool_selection(
    tools: nanocode_tool_host::Tools,
) -> Result<nanocode_tool_host::Tools, nanocode_tool_host::ToolsBuildError> {
    if tools.has_code_mode_host() {
        return tools.with_builtin_names(std::iter::empty::<Box<str>>());
    }
    let names = [
        (tools.workspace_enabled(), StandardTool::ExecCommand.name()),
        (tools.workspace_enabled(), StandardTool::WriteStdin.name()),
        (tools.workspace_enabled(), StandardTool::UpdatePlan.name()),
        (tools.workspace_enabled(), StandardTool::ApplyPatch.name()),
        (tools.workspace_enabled(), StandardTool::ViewImage.name()),
        (tools.web_search_enabled(), "web__run"),
        (tools.image_generation_enabled(), "image_gen__imagegen"),
    ];
    tools.with_builtin_names(
        names
            .into_iter()
            .filter_map(|(enabled, name)| enabled.then_some(name)),
    )
}
