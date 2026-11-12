use std::sync::Arc;

use nanocode_tools::ToolDefinition;

use super::{Tool, ToolContext, ToolInput, ToolOutput, ToolResult};

struct GatewayUnsupportedTool {
    inner: Arc<dyn Tool>,
    message: String,
}

pub(super) fn wrap(handlers: Vec<Arc<dyn Tool>>, gateway: &str) -> Vec<Arc<dyn Tool>> {
    handlers
        .into_iter()
        .map(|inner| {
            let message = format!(
                "{} is unavailable through {gateway}: the built-in adapter is not configured for auxiliary endpoints or authentication through gateways. No request was sent. Register an explicit adapter tool to provide it.",
                inner.definition().name()
            );
            Arc::new(GatewayUnsupportedTool { inner, message }) as Arc<dyn Tool>
        })
        .collect()
}

impl Tool for GatewayUnsupportedTool {
    fn definition(&self) -> ToolDefinition {
        self.inner.definition()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        self.inner.supports_parallel_tool_calls()
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _input: ToolInput,
        _context: ToolContext<'life1>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = { Ok(ToolOutput::error(self.message.clone())) };
            output
        })
    }
}
