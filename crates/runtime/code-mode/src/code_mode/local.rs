use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use nanocode_sync::Atom;
use nanocode_sync::FutureExt;
use serde_json::{Value, json, value::to_raw_value};

use super::{
    CellJournals, CodeModeExecution, CodeModeObserver, CodeModeRuntime, IgnoreCodeModeUpdates,
};
use crate::{
    CodeModeToolset, Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput,
    embedded::{
        CodeModeHost, CodeModeHostError, EmbeddedToolMode, HandCalls, HostFuture, OwnedToolContext,
        declares_hand, take_hand, tool_not_available,
    },
};

#[derive(Default)]
pub struct LocalCells {
    sessions: Atom<HashMap<Arc<str>, Arc<LocalSession>>>,
    prelude: Option<Arc<str>>,
    host_tools: Vec<Arc<dyn Tool>>,
    hands: Atom<Option<Hands>>,
    journals: Option<CellJournals>,
    default_yield: Option<std::time::Duration>,
}

type Hands = (Arc<str>, Arc<dyn HandCalls>);

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

    #[must_use]
    pub fn host_tools(mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Self {
        self.host_tools.extend(tools);
        self
    }

    #[must_use]
    pub fn journals(mut self, journals: CellJournals) -> Self {
        self.journals = Some(journals);
        self
    }

    #[must_use]
    pub const fn default_yield(mut self, after: std::time::Duration) -> Self {
        self.default_yield = Some(after);
        self
    }

    fn toolset(&self, tools: Vec<Arc<dyn Tool>>) -> Arc<dyn CodeModeToolset> {
        Arc::new(LocalTools::new(
            tools,
            &self.host_tools,
            Option::clone(&self.hands.get()),
        ))
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
                let mut runtime = match &self.journals {
                    Some(journals) => runtime.with_journals(journals.clone()),
                    None => runtime,
                };
                if let Some(after) = self.default_yield {
                    runtime.set_default_yield(after);
                }
                Arc::new(LocalSession { runtime, turn })
            }))
        })
    }

    fn existing(&self, session_id: &str) -> Option<Arc<LocalSession>> {
        self.sessions
            .read(|sessions| sessions.get(session_id).cloned())
    }
}

struct LocalTools {
    tools: Vec<(Arc<dyn Tool>, ToolDefinition)>,
    host: Vec<(Arc<dyn Tool>, ToolDefinition)>,
    hands: Option<Hands>,
}

impl LocalTools {
    fn new(tools: Vec<Arc<dyn Tool>>, host_tools: &[Arc<dyn Tool>], hands: Option<Hands>) -> Self {
        let defined = |tool: Arc<dyn Tool>| {
            let definition = tool.definition();
            (tool, definition)
        };
        Self {
            tools: tools.into_iter().map(defined).collect(),
            host: host_tools.iter().cloned().map(defined).collect(),
            hands,
        }
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
        self.tools
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

    fn shared_nested_tool_metadata(&self) -> Arc<[Value]> {
        static LIVE: LazyLock<Atom<Vec<Weak<[Value]>>>> = LazyLock::new(Atom::default);
        let metadata = self.nested_tool_metadata();
        let live = LIVE.read(|live| {
            live.iter()
                .filter_map(Weak::upgrade)
                .find(|shared| **shared == *metadata)
        });
        if let Some(shared) = live {
            return shared;
        }
        let shared: Arc<[Value]> = metadata.into();
        let weak = Arc::downgrade(&shared);
        LIVE.update(|live| {
            live.retain(|entry| entry.strong_count() > 0);
            live.push(Weak::clone(&weak));
        });
        shared
    }

    fn execute_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput> {
        let found = find(&self.tools, name);
        let Some((here, hands)) = &self.hands else {
            return execute(found, name, input, context);
        };
        let declared = found.is_some_and(|(_, definition)| declares_hand(definition));
        match take_hand(declared, input) {
            Err(output) => Box::pin(async move { output }),
            Ok((input, Some(hand))) if hand != here.as_ref() && !hands.here(&hand) => {
                Box::pin(async move { hands.call(&hand, name, input, context).await })
            }
            Ok((input, _)) => execute(found, name, input, context),
        }
    }

    fn execute_host_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput> {
        let found = find(&self.host, name).or_else(|| find(&self.tools, name));
        execute(found, name, input, context)
    }

    fn recover_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> Option<HostFuture<'a, Option<ToolOutput>>> {
        let found = find(&self.tools, name);
        let Some((here, hands)) = &self.hands else {
            return recover(found, name, input, context);
        };
        let declared = found.is_some_and(|(_, definition)| declares_hand(definition));
        match take_hand(declared, input) {
            Ok((_, Some(hand))) if hand != here.as_ref() && !hands.here(&hand) => {
                Some(Box::pin(async move { hands.recover(&hand, context).await }))
            }
            Ok((input, _)) => recover(found, name, input, context),
            Err(_) => None,
        }
    }
}

fn find<'a>(
    tools: &'a [(Arc<dyn Tool>, ToolDefinition)],
    name: &str,
) -> Option<&'a (Arc<dyn Tool>, ToolDefinition)> {
    tools
        .iter()
        .rev()
        .find(|(_, definition)| definition.name() == name)
}

fn recover<'a>(
    found: Option<&'a (Arc<dyn Tool>, ToolDefinition)>,
    name: &'a str,
    input: Value,
    context: ToolContext<'a>,
) -> Option<HostFuture<'a, Option<ToolOutput>>> {
    let (tool, definition) = found?;
    tool.recover(tool_input(definition, name, input).ok()?, context)
}

fn tool_input(
    definition: &ToolDefinition,
    name: &str,
    input: Value,
) -> Result<ToolInput, ToolOutput> {
    match definition {
        ToolDefinition::Function { .. } if !input.is_object() => Err(ToolOutput::error(format!(
            "nested function tool {name} requires an object argument"
        ))),
        ToolDefinition::Function { .. } => to_raw_value(&input)
            .map(ToolInput::Function)
            .map_err(|error| ToolOutput::error(format!("failed to encode {name} input: {error}"))),
        ToolDefinition::Custom { .. } => match &input {
            Value::String(input) => Ok(ToolInput::Freeform(input.clone())),
            Value::Object(fields) if fields.len() == 1 => fields
                .get("input")
                .and_then(Value::as_str)
                .map(|input| ToolInput::Freeform(input.to_owned()))
                .ok_or_else(|| {
                    ToolOutput::error(format!(
                        "nested freeform tool {name} requires a string argument or {{ input }}"
                    ))
                }),
            _ => Err(ToolOutput::error(format!(
                "nested freeform tool {name} requires a string argument or {{ input }}"
            ))),
        },
        _ => Err(ToolOutput::error(format!(
            "unsupported nested tool call: {name}"
        ))),
    }
}

fn execute<'a>(
    found: Option<&'a (Arc<dyn Tool>, ToolDefinition)>,
    name: &'a str,
    input: Value,
    context: ToolContext<'a>,
) -> HostFuture<'a, ToolOutput> {
    Box::pin(async move {
        let Some((tool, definition)) = found else {
            return tool_not_available(name);
        };
        let input = match tool_input(definition, name, input) {
            Ok(input) => input,
            Err(output) => return output,
        };
        match tool.execute(input, context).await {
            Ok(output) => output,
            Err(error) => ToolOutput::error(error.to_string()),
        }
    })
}

impl CodeModeHost for LocalCells {
    fn tool_mode(&self) -> EmbeddedToolMode {
        EmbeddedToolMode::CodeOnly
    }

    fn supports_cells(&self) -> bool {
        true
    }

    fn quiet(&self, session_id: &str) -> bool {
        self.existing(session_id)
            .is_none_or(|session| session.runtime.quiet())
    }

    fn tool_definitions(&self, _: &str) -> Result<Vec<ToolDefinition>, CodeModeHostError> {
        Ok(Vec::new())
    }

    fn describe(&self) -> Option<crate::remote::HandDescription> {
        Some(crate::remote::HandDescription::default())
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
        let tools = self.toolset(tools);
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

    fn recover<'a>(
        &'a self,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Option<CodeModeExecution>, CodeModeHostError>> {
        let session = self.session(context.session_id());
        let tools = self.toolset(Vec::new());
        let context = OwnedToolContext::from_context(context);
        Box::pin(async move {
            Ok(session
                .runtime
                .recover_with_updates(tools, context, &mut IgnoreCodeModeUpdates)
                .await)
        })
    }

    fn adopt<'a>(
        &'a self,
        context: ToolContext<'a>,
        tools: Vec<Arc<dyn Tool>>,
    ) -> HostFuture<'a, ()> {
        let session = self.session(context.session_id());
        let tools = self.toolset(tools);
        let context = OwnedToolContext::from_context(context);
        Box::pin(async move { session.runtime.adopt(tools, &context).await })
    }

    fn route_hands(&self, name: &str, hands: Arc<dyn HandCalls>) {
        self.hands.set(Some((Arc::from(name), hands)));
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
