use std::sync::Arc;

use nanocode_tool_contracts::{Tool, ToolCollection};

use crate::AgentHandle;

pub type AgentToolsFactory = Arc<dyn Fn(&AgentHandle) -> Vec<Arc<dyn Tool>> + Send + Sync>;

#[derive(Clone, Default)]
pub struct RuntimeTools {
    shared: Vec<Arc<dyn Tool>>,
    per_agent: Vec<AgentToolsFactory>,
}

impl RuntimeTools {
    #[must_use]
    pub fn shared(mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Self {
        self.shared.extend(tools);
        self
    }

    #[must_use]
    pub fn per_agent(mut self, factory: AgentToolsFactory) -> Self {
        self.per_agent.push(factory);
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shared.is_empty() && self.per_agent.is_empty()
    }

    #[must_use]
    pub fn for_agent(&self, handle: &AgentHandle) -> Vec<Arc<dyn Tool>> {
        self.shared
            .iter()
            .cloned()
            .chain(self.per_agent.iter().flat_map(|factory| factory(handle)))
            .collect()
    }

    pub fn install<T: ToolCollection>(
        &self,
        tools: T,
        handle: &AgentHandle,
    ) -> Result<T, T::Error> {
        tools.add_shared_tools(self.for_agent(handle))
    }
}
