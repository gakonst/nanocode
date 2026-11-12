use std::{sync::Arc, time::Duration};

use nanocode_tool_provider::{DynamicToolProvider, ToolExposure};
use nanocode_tools::{Tool, ToolContext, ToolDefinition, ToolOutput};
use serde_json::Value;

pub use nanocode_mcp_tools::*;

pub struct Mcp(nanocode_mcp_tools::Mcp);

#[derive(Default)]
pub struct McpBuilder(nanocode_mcp_tools::McpBuilder);

impl Mcp {
    #[must_use]
    pub fn builder() -> McpBuilder {
        McpBuilder::default()
    }

    #[must_use]
    pub fn handle(&self) -> McpHandle {
        self.0.handle()
    }

    pub(crate) async fn prepared_snapshot(
        &self,
        max_timeout: Duration,
    ) -> Result<Vec<PreparedMcpTool>, String> {
        self.0.prepared_snapshot(max_timeout).await
    }
}

impl McpBuilder {
    #[must_use]
    pub fn oauth_store(self, store: Arc<dyn McpOAuthStore>) -> Self {
        Self(self.0.oauth_store(store))
    }

    #[must_use]
    pub fn server(self, name: impl Into<String>, server: McpServer) -> Self {
        Self(self.0.server(name, server))
    }

    pub fn build(self) -> Result<Mcp, McpBuildError> {
        self.0.build().map(Mcp)
    }
}

impl crate::ToolSource for Mcp {
    fn install(self, builder: crate::ToolsBuilder) -> crate::ToolsBuilder {
        builder.attachable_provider(Arc::new(self))
    }
}

impl DynamicToolProvider for Mcp {
    fn start(&self) {
        self.0.start();
    }

    fn bind_session(&self, session: &crate::SessionEnvironment) {
        self.0.bind_session(session);
    }

    fn direct_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.0.direct_tools()
    }

    fn direct_tools_for_exposure(&self, exposure: ToolExposure) -> Vec<Arc<dyn Tool>> {
        self.0.direct_tools_for_exposure(exposure)
    }

    fn available_definitions(&self) -> Vec<ToolDefinition> {
        self.0.available_definitions()
    }

    fn model_catalog_visible(&self) -> bool {
        self.0.model_catalog_visible()
    }

    fn code_mode_tool_summaries(&self) -> Vec<(String, String)> {
        self.0.code_mode_tool_summaries()
    }

    fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    fn supports_parallel_tool_calls(&self, name: &str) -> bool {
        self.0.supports_parallel_tool_calls(name)
    }

    fn execute<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        name: &'life1 str,
        input: Value,
        context: ToolContext<'life2>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<ToolOutput>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Option<ToolOutput> = { self.0.execute(name, input, context).await };
            output
        })
    }

    fn resumes(&self, name: &str) -> bool {
        self.0.resumes(name)
    }

    fn recover<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        name: &'life1 str,
        input: Value,
        context: ToolContext<'life2>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<ToolOutput>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Option<ToolOutput> = { self.0.recover(name, input, context).await };
            output
        })
    }
}
