use std::sync::Arc;

use nanocode_tools::ToolDefinition;

use super::{Tool, ToolContext, ToolInput, ToolOutput, ToolResult};

struct GatewayUnsupportedTool {
    inner: Arc<dyn Tool>,
    message: String,
}

pub(super) fn wrap(handlers: Vec<Arc<dyn Tool>>, gateway: &'static str) -> Vec<Arc<dyn Tool>> {
    handlers
        .into_iter()
        .map(|inner| {
            let message = format!(
                "{} is unavailable through the {gateway} Responses gateway: the built-in adapter is not configured for auxiliary endpoints or authentication through Responses gateways. No request was sent. Register an explicit adapter tool to provide it.",
                inner.definition().name()
            );
            Arc::new(GatewayUnsupportedTool { inner, message }) as Arc<dyn Tool>
        })
        .collect()
}

#[async_trait::async_trait]
impl Tool for GatewayUnsupportedTool {
    fn definition(&self) -> ToolDefinition {
        self.inner.definition()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        self.inner.supports_parallel_tool_calls()
    }

    async fn execute(&self, _input: ToolInput, _context: ToolContext<'_>) -> ToolResult {
        Ok(ToolOutput::error(self.message.clone()))
    }
}
