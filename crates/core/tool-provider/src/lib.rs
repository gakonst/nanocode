#[cfg(not(target_family = "wasm"))]
#[cfg(not(target_family = "wasm"))]
pub use nanocode_home::SessionEnvironment;
#[cfg(not(target_family = "wasm"))]
use nanocode_tool_contracts::{Tool, ToolContext, ToolDefinition, ToolOutput};
#[cfg(not(target_family = "wasm"))]
use serde_json::Value;
#[cfg(not(target_family = "wasm"))]
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToolExposure {
    #[default]
    CodeModeOnly,

    DirectAndCodeMode,

    DirectOnly,

    Hidden,
}

impl ToolExposure {
    #[cfg(not(target_family = "wasm"))]
    pub const fn is_direct(self) -> bool {
        matches!(self, Self::DirectAndCodeMode | Self::DirectOnly)
    }

    pub const fn is_available_in_code_mode(self) -> bool {
        matches!(self, Self::CodeModeOnly | Self::DirectAndCodeMode)
    }
}

#[cfg(not(target_family = "wasm"))]
pub trait DynamicToolProvider: Send + Sync {
    fn start(&self) {}

    fn bind_session(&self, _session: &crate::SessionEnvironment) {}

    fn direct_tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }

    fn direct_tools_for_exposure(&self, _exposure: ToolExposure) -> Vec<Arc<dyn Tool>> {
        self.direct_tools()
    }

    fn available_definitions(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }

    fn model_catalog_visible(&self) -> bool {
        true
    }

    fn catalog_revision(&self) -> Option<u64> {
        None
    }

    fn code_mode_tool_summaries(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn contains(&self, name: &str) -> bool {
        self.available_definitions()
            .iter()
            .any(|definition| definition.name() == name)
    }

    fn supports_parallel_tool_calls(&self, _name: &str) -> bool {
        false
    }

    fn resolve<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _name: &'life1 str,
    ) -> std::pin::Pin<Box<dyn Future<Output = bool> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: bool = { false };
            output
        })
    }

    fn execute<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _name: &'life1 str,
        _input: Value,
        _context: ToolContext<'life2>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<ToolOutput>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Option<ToolOutput> = { None };
            output
        })
    }

    fn resumes(&self, _name: &str) -> bool {
        false
    }

    fn recover<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _name: &'life1 str,
        _input: Value,
        _context: ToolContext<'life2>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<ToolOutput>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Option<ToolOutput> = { None };
            output
        })
    }
}
