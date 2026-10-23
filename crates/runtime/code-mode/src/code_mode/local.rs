use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::FutureExt;
use nanocode_sync::Atom;
use serde_json::{Value, json, value::to_raw_value};

use super::{CodeModeExecution, CodeModeObserver, CodeModeRuntime, IgnoreCodeModeUpdates};
use crate::{
    CodeModeToolset, Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput,
    embedded::{CodeModeHost, CodeModeHostError, EmbeddedToolMode, HostFuture, OwnedToolContext},
};

#[derive(Default)]
pub struct LocalCells {
    sessions: Atom<HashMap<Arc<str>, Arc<LocalSession>>>,
    prelude: Option<Arc<str>>,
}

struct LocalSession {
    runtime: CodeModeRuntime,
    turn: Arc<AtomicU64>,
}

impl LocalCells {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn prelude(mut self, source: impl Into<Arc<str>>) -> Self {
        self.prelude = Some(source.into());
        self
    }

    fn session(&self, session_id: &str) -> Arc<LocalSession> {
        if let Some(session) = self.existing(session_id) {
            return session;
        }
        self.sessions.update(|sessions| {
            Arc::clone(sessions.entry(Arc::from(session_id)).or_insert_with(|| {
                let turn = Arc::new(AtomicU64::new(0));
                let runtime = CodeModeRuntime::new_with_turn(Arc::clone(&turn));
                let runtime = match &self.prelude {
                    Some(prelude) => runtime.with_prelude(Arc::clone(prelude)),
                    None => runtime,
                };
                Arc::new(LocalSession { runtime, turn })
            }))
        })
    }

    fn existing(&self, session_id: &str) -> Option<Arc<LocalSession>> {
        self.sessions
            .read(|sessions| sessions.get(session_id).cloned())
    }
}

struct LocalTools(Vec<(Arc<dyn Tool>, ToolDefinition)>);

impl LocalTools {
    fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        Self(
            tools
                .into_iter()
                .map(|tool| {
                    let definition = tool.definition();
                    (tool, definition)
                })
                .collect(),
        )
    }
}

fn identifier(name: &str) -> String {
    name.chars()
        .enumerate()
        .map(|(index, character)| {
            let valid = character == '_'
                || character == '$'
                || if index == 0 {
                    character.is_ascii_alphabetic()
                } else {
                    character.is_ascii_alphanumeric()
                };
            if valid { character } else { '_' }
        })
        .collect()
}

impl CodeModeToolset for LocalTools {
    fn nested_tool_metadata(&self) -> Vec<Value> {
        self.0
            .iter()
            .filter_map(|(_, definition)| {
                let kind = match definition {
                    ToolDefinition::Function { .. } => "function",
                    ToolDefinition::Custom { .. } => "freeform",
                    _ => return None,
                };
                let name = definition.name();
                Some(json!({
                    "name": identifier(name),
                    "tool_name": name,
                    "description": definition.description(),
                    "kind": kind,
                    "input_schema": definition.parameters().map(|schema| schema.as_value()),
                    "output_schema": definition.output_schema().map(|schema| schema.as_value()),
                }))
            })
            .collect()
    }

    fn execute_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Some((tool, definition)) = self
                .0
                .iter()
                .find(|(_, definition)| definition.name() == name)
            else {
                return ToolOutput::error(format!("unsupported nested tool call: {name}"));
            };
            let input = match definition {
                ToolDefinition::Function { .. } if !input.is_object() => {
                    return ToolOutput::error(format!(
                        "nested function tool {name} requires an object argument"
                    ));
                }
                ToolDefinition::Function { .. } => match to_raw_value(&input) {
                    Ok(input) => ToolInput::Function(input),
                    Err(error) => {
                        return ToolOutput::error(format!(
                            "failed to encode {name} input: {error}"
                        ));
                    }
                },
                ToolDefinition::Custom { .. } => match &input {
                    Value::String(input) => ToolInput::Freeform(input.clone()),
                    Value::Object(fields) if fields.len() == 1 => {
                        match fields.get("input").and_then(Value::as_str) {
                            Some(input) => ToolInput::Freeform(input.to_owned()),
                            None => {
                                return ToolOutput::error(format!(
                                    "nested freeform tool {name} requires a string argument or {{ input }}"
                                ));
                            }
                        }
                    }
                    _ => {
                        return ToolOutput::error(format!(
                            "nested freeform tool {name} requires a string argument or {{ input }}"
                        ));
                    }
                },
                _ => return ToolOutput::error(format!("unsupported nested tool call: {name}")),
            };
            match tool.execute(input, context).await {
                Ok(output) => output,
                Err(error) => ToolOutput::error(error.to_string()),
            }
        })
    }
}

impl CodeModeHost for LocalCells {
    fn tool_mode(&self) -> EmbeddedToolMode {
        EmbeddedToolMode::CodeOnly
    }

    fn supports_cells(&self) -> bool {
        true
    }

    fn tool_definitions(&self, _: &str) -> Result<Vec<ToolDefinition>, CodeModeHostError> {
        Ok(Vec::new())
    }

    fn execute<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        self.execute_with_local_tools(source, context, Vec::new(), None)
    }

    fn execute_with_updates<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        self.execute_with_local_tools(source, context, Vec::new(), Some(observer))
    }

    fn execute_with_local_tools<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        tools: Vec<Arc<dyn Tool>>,
        observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        let session = self.session(context.session_id());
        let tools: Arc<dyn CodeModeToolset> = Arc::new(LocalTools::new(tools));
        let context = OwnedToolContext::from_context(context);
        Box::pin(async move {
            let mut ignore = IgnoreCodeModeUpdates;
            let observer = match observer {
                Some(observer) => observer,
                None => &mut ignore,
            };
            Ok(session
                .runtime
                .execute_with_updates(source, tools, context, observer)
                .await)
        })
    }

    fn wait_with_updates<'a>(
        &'a self,
        input: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        let session = self.session(context.session_id());
        Box::pin(async move {
            Ok(session
                .runtime
                .wait_with_updates(input, context, observer)
                .await)
        })
    }

    fn begin_turn(&self, session_id: &str) {
        if let Some(session) = self.existing(session_id) {
            session.turn.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn preempt_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        if let Some(session) = self.existing(session_id) {
            session.runtime.control().preempt_turn();
        }
        Box::pin(async { Ok(()) })
    }

    fn settle_preempt(&self, session_id: &str) {
        if let Some(session) = self.existing(session_id) {
            session.runtime.control().settle_preempt();
        }
    }

    fn detach_turn(
        &self,
        session_id: &str,
        observer: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) {
        if let Some(session) = self.existing(session_id) {
            let turn = session.turn.load(Ordering::Acquire);
            let _ = session
                .runtime
                .control()
                .detach_turn(turn, observer)
                .now_or_never();
        }
    }

    fn cancel_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        let session = self.existing(session_id);
        Box::pin(async move {
            if let Some(session) = session {
                let turn = session.turn.load(Ordering::Acquire);
                session.runtime.control().terminate_turn(turn, None).await;
            }
            Ok(())
        })
    }

    fn cancel_turn_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        let session = self.existing(session_id);
        Box::pin(async move {
            if let Some(session) = session {
                let turn = session.turn.load(Ordering::Acquire);
                session
                    .runtime
                    .control()
                    .terminate_turn(turn, Some(observer))
                    .await;
            }
            Ok(())
        })
    }

    fn cancel<'a>(&'a self, session_id: &'a str) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        let session = self.sessions.update(|sessions| sessions.remove(session_id));
        Box::pin(async move {
            if let Some(session) = session {
                let _ = session.runtime.control().terminate_all(None).await;
            }
            Ok(())
        })
    }

    fn cancel_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        let session = self.sessions.update(|sessions| sessions.remove(session_id));
        Box::pin(async move {
            if let Some(session) = session {
                let _ = session
                    .runtime
                    .control()
                    .terminate_all(Some(observer))
                    .await;
            }
            Ok(())
        })
    }
}
