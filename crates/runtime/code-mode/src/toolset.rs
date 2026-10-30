use nanocode_tool_contracts::{ToolContext, ToolOutput, ToolOutputContent, ToolProcessTraceWire};
use serde_json::Value;

use crate::embedded::{HostFuture, NestedToolCall};

pub struct CodeModeToolObservation<'a> {
    pub call: &'a NestedToolCall,
    pub process_trace: Option<&'a ToolProcessTraceWire>,
}

pub trait CodeModeToolset: Send + Sync {
    fn nested_tool_metadata(&self) -> Vec<Value>;

    fn execute_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput>;

    fn execute_host_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput> {
        self.execute_nested(name, input, context)
    }

    fn recover_nested<'a>(
        &'a self,
        _name: &'a str,
        _input: Value,
        _context: ToolContext<'a>,
    ) -> Option<HostFuture<'a, Option<ToolOutput>>> {
        None
    }

    fn cell_observed(&self, _origin_call_id: &str, _observed: bool) {}

    fn append_observation_content(
        &self,
        _calls: &[CodeModeToolObservation<'_>],
        _content: &mut Vec<ToolOutputContent>,
    ) {
    }
}
