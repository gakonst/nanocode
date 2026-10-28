mod input;
mod types;

use std::{error::Error, fmt, future::Future, pin::Pin};

use serde_json::Value;

use crate::{ToolContext, ToolDefinition, ToolInput, ToolOutput};

pub use input::{prepare_output_images, prepare_user_input};
pub(crate) use types::shared_history;
pub use types::{
    CodeModeCell, CodeModeExecution, CodeModeNotification, CodeModeObserver, CodeModeUpdate,
    NestedToolCall, OwnedToolContext,
};

#[cfg(not(target_family = "wasm"))]
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[cfg(target_family = "wasm")]
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Debug)]
pub struct CodeModeHostError {
    message: Box<str>,
    unsent: bool,
    refused: Box<[u64]>,
}

impl CodeModeHostError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into().into_boxed_str(),
            unsent: false,
            refused: Box::default(),
        }
    }

    #[must_use]
    pub fn unsent(message: impl Into<String>) -> Self {
        Self {
            message: message.into().into_boxed_str(),
            unsent: true,
            refused: Box::default(),
        }
    }

    #[must_use]
    pub fn refused(message: impl Into<String>, ids: Vec<u64>) -> Self {
        Self {
            message: message.into().into_boxed_str(),
            unsent: false,
            refused: ids.into_boxed_slice(),
        }
    }

    #[must_use]
    pub const fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn is_unsent(&self) -> bool {
        self.unsent
    }

    #[must_use]
    pub fn refused_requests(&self) -> &[u64] {
        &self.refused
    }
}

impl fmt::Display for CodeModeHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CodeModeHostError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EmbeddedToolMode {
    #[default]
    Code,

    CodeOnly,

    Direct,
}

pub trait HandCalls: Send + Sync + 'static {
    fn here(&self, hand: &str) -> bool;

    fn call<'a>(
        &'a self,
        hand: &'a str,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput>;
}

pub trait CodeModeHost: Send + Sync + 'static {
    fn tool_mode(&self) -> EmbeddedToolMode {
        EmbeddedToolMode::Code
    }

    fn supports_cells(&self) -> bool {
        false
    }

    fn wait_with_updates<'a>(
        &'a self,
        _input: &'a str,
        _context: ToolContext<'a>,
        _observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(async {
            Err(CodeModeHostError::new(
                "embedded host does not support resumable cells",
            ))
        })
    }

    fn tool_definitions(&self, session_id: &str) -> Result<Vec<ToolDefinition>, CodeModeHostError>;

    fn execute<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>>;

    fn execute_with_updates<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(async move {
            let execution = self.execute(source, context).await?;
            for call in &execution.nested_calls {
                observer.update(CodeModeUpdate::NestedCallStarted {
                    call_id: &call.call_id,
                    name: &call.name,
                    input: &call.input,
                });
                observer.update(CodeModeUpdate::NestedCallCompleted(call));
            }
            Ok(execution)
        })
    }

    fn execute_with_local_tools<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        tools: Vec<std::sync::Arc<dyn crate::Tool>>,
        observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(async move {
            if !tools.is_empty() {
                return Err(CodeModeHostError::new(
                    "embedded host does not support nested local tools",
                ));
            }
            match observer {
                Some(observer) => self.execute_with_updates(source, context, observer).await,
                None => self.execute(source, context).await,
            }
        })
    }

    fn execute_tool<'a>(
        &'a self,
        name: &'a str,
        _input: ToolInput,
        _context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<ToolOutput, CodeModeHostError>> {
        Box::pin(async move {
            Err(CodeModeHostError::new(format!(
                "direct embedded tool `{name}` is unavailable"
            )))
        })
    }

    fn call<'a>(
        &'a self,
        name: &'a str,
        _input: Value,
        _context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<ToolOutput, CodeModeHostError>> {
        Box::pin(async move {
            Err(CodeModeHostError::new(format!(
                "this host cannot run `{name}` for another hand"
            )))
        })
    }

    fn recover<'a>(
        &'a self,
        _context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Option<CodeModeExecution>, CodeModeHostError>> {
        Box::pin(async { Ok(None) })
    }

    fn route_hands(&self, _name: &str, _hands: std::sync::Arc<dyn HandCalls>) {}

    fn describe(&self) -> Option<crate::remote::HandDescription> {
        None
    }

    fn runs_cells(&self) -> bool {
        true
    }

    fn capabilities(
        &self,
    ) -> HostFuture<'_, Result<Vec<crate::remote::HandCapability>, CodeModeHostError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn request_capability<'a>(
        &'a self,
        name: &'a str,
        _reason: &'a str,
    ) -> HostFuture<'a, Result<crate::remote::HandPermission, CodeModeHostError>> {
        Box::pin(async move {
            Err(CodeModeHostError::new(format!(
                "this hand offers no capability named `{name}`; nothing was requested"
            )))
        })
    }

    fn bind_session(&self, _session_id: &str, _backend: &str) {}

    fn link_session(&self, _session_id: &str, _root_session_id: &str) {}

    fn begin_turn(&self, _session_id: &str) {}

    fn detach_turn(
        &self,
        _session_id: &str,
        _observer: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) {
    }

    fn preempt_turn<'a>(
        &'a self,
        _session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async { Ok(()) })
    }

    fn settle_preempt(&self, _session_id: &str) {}

    fn cancel_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        self.cancel(session_id)
    }

    fn cancel_turn_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        _observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        self.cancel_turn(session_id)
    }

    fn cancel_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        _observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        self.cancel(session_id)
    }

    fn cancel<'a>(&'a self, _session_id: &'a str) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async { Ok(()) })
    }
}

impl<T: CodeModeHost + ?Sized> CodeModeHost for std::sync::Arc<T> {
    fn tool_mode(&self) -> EmbeddedToolMode {
        (**self).tool_mode()
    }
    fn supports_cells(&self) -> bool {
        (**self).supports_cells()
    }
    fn tool_definitions(&self, session_id: &str) -> Result<Vec<ToolDefinition>, CodeModeHostError> {
        (**self).tool_definitions(session_id)
    }
    fn execute<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        (**self).execute(source, context)
    }
    fn execute_with_updates<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        (**self).execute_with_updates(source, context, observer)
    }
    fn execute_with_local_tools<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        tools: Vec<std::sync::Arc<dyn crate::Tool>>,
        observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        (**self).execute_with_local_tools(source, context, tools, observer)
    }
    fn wait_with_updates<'a>(
        &'a self,
        input: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        (**self).wait_with_updates(input, context, observer)
    }
    fn execute_tool<'a>(
        &'a self,
        name: &'a str,
        input: ToolInput,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<ToolOutput, CodeModeHostError>> {
        (**self).execute_tool(name, input, context)
    }
    fn call<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<ToolOutput, CodeModeHostError>> {
        (**self).call(name, input, context)
    }
    fn recover<'a>(
        &'a self,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Option<CodeModeExecution>, CodeModeHostError>> {
        (**self).recover(context)
    }
    fn route_hands(&self, name: &str, hands: std::sync::Arc<dyn HandCalls>) {
        (**self).route_hands(name, hands);
    }
    fn describe(&self) -> Option<crate::remote::HandDescription> {
        (**self).describe()
    }
    fn runs_cells(&self) -> bool {
        (**self).runs_cells()
    }
    fn capabilities(
        &self,
    ) -> HostFuture<'_, Result<Vec<crate::remote::HandCapability>, CodeModeHostError>> {
        (**self).capabilities()
    }
    fn request_capability<'a>(
        &'a self,
        name: &'a str,
        reason: &'a str,
    ) -> HostFuture<'a, Result<crate::remote::HandPermission, CodeModeHostError>> {
        (**self).request_capability(name, reason)
    }
    fn bind_session(&self, session_id: &str, backend: &str) {
        (**self).bind_session(session_id, backend);
    }
    fn link_session(&self, session_id: &str, root_session_id: &str) {
        (**self).link_session(session_id, root_session_id);
    }
    fn begin_turn(&self, session_id: &str) {
        (**self).begin_turn(session_id);
    }
    fn detach_turn(
        &self,
        session_id: &str,
        observer: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) {
        (**self).detach_turn(session_id, observer);
    }
    fn preempt_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        (**self).preempt_turn(session_id)
    }
    fn settle_preempt(&self, session_id: &str) {
        (**self).settle_preempt(session_id);
    }
    fn cancel_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        (**self).cancel_turn(session_id)
    }
    fn cancel_turn_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        (**self).cancel_turn_with_updates(session_id, observer)
    }
    fn cancel_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        (**self).cancel_with_updates(session_id, observer)
    }
    fn cancel<'a>(&'a self, session_id: &'a str) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        (**self).cancel(session_id)
    }
}
