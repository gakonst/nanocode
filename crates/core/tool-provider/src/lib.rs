#[cfg(not(target_family = "wasm"))]
use async_trait::async_trait;
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
#[async_trait]
pub trait DynamicToolProvider: Send + Sync {
    fn start(&self);

    fn bind_session(&self, _session: &crate::SessionEnvironment) {}

    fn direct_tools(&self) -> Vec<Arc<dyn Tool>>;

    fn direct_tools_for_exposure(&self, _exposure: ToolExposure) -> Vec<Arc<dyn Tool>> {
        self.direct_tools()
    }

    fn available_definitions(&self) -> Vec<ToolDefinition>;

    fn model_catalog_visible(&self) -> bool {
        true
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

    /// Called for a nested call to `name` that neither the host nor any
    /// provider lists, before the host rejects it with `TOOL_NOT_AVAILABLE`.
    /// A Code Mode cell fixes its tool names when it starts and sends a name
    /// missing from them to the host (`code-tools.mjs`), so a provider that
    /// learns its tools in the background can make one callable now: register
    /// it so `available_definitions` lists it and return `true`, and the
    /// host runs the call through `execute`. Return `false` when it is not
    /// yours or can't be found. The host asks each provider in turn and stops
    /// at the first `true`. Never changes what the model's catalog shows.
    async fn resolve(&self, _name: &str) -> bool {
        false
    }

    async fn execute(
        &self,
        name: &str,
        input: Value,
        context: ToolContext<'_>,
    ) -> Option<ToolOutput>;
}
