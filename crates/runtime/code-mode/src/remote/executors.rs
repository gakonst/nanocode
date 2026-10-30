use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Weak},
};

use nanocode_sync::Atom;

use serde_json::{Value, json};

use crate::{
    Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult,
    embedded::{
        CodeModeExecution, CodeModeHost, CodeModeHostError, CodeModeObserver, EmbeddedToolMode,
        HandCalls, HostFuture,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutorSelection {
    Local,
    Executor(Arc<str>),
}

#[derive(Clone)]
pub struct HandRoute {
    pub name: Arc<str>,
    pub host: Option<Arc<dyn CodeModeHost>>,
}

#[derive(Clone)]
pub enum Placement {
    Local,
    Hand(HandRoute),
}

#[derive(Clone, Default)]
pub struct CodeModeExecutors {
    state: Arc<Atom<State>>,
    changed: Arc<tokio::sync::Notify>,
}

#[derive(Clone, Default)]
struct State {
    executors: BTreeMap<Arc<str>, Arc<dyn CodeModeHost>>,
    selections: HashMap<Box<str>, ExecutorSelection>,
    roots: HashMap<Box<str>, Box<str>>,
    backends: HashMap<Box<str>, Box<str>>,
    default: Option<Arc<str>>,
    fallback: Option<Arc<str>>,
    awaiting: bool,
    sessions: HashMap<Box<str>, SessionPlaces>,
}

#[derive(Clone, Default)]
struct SessionPlaces {
    touched: Vec<Arc<str>>,
    cells: Vec<(Box<str>, Option<Arc<str>>)>,
}

impl State {
    fn resolve(&self, session_id: &str) -> ExecutorSelection {
        self.selections
            .get(session_id)
            .or_else(|| {
                self.roots
                    .get(session_id)
                    .and_then(|root| self.selections.get(root.as_ref()))
            })
            .cloned()
            .or_else(|| {
                self.default
                    .clone()
                    .filter(|name| {
                        self.executors
                            .get(name)
                            .is_none_or(|host| host.runs_cells())
                    })
                    .map(ExecutorSelection::Executor)
            })
            .or_else(|| self.fallback.clone().map(ExecutorSelection::Executor))
            .unwrap_or(ExecutorSelection::Local)
    }

    fn awaits_replacement(&self, session_id: &str) -> bool {
        self.awaiting
            && !self.selections.contains_key(session_id)
            && !self
                .roots
                .get(session_id)
                .is_some_and(|root| self.selections.contains_key(root.as_ref()))
            && self
                .default
                .as_ref()
                .is_none_or(|name| !self.executors.contains_key(name))
    }

    fn placement(&self, session_id: &str) -> Placement {
        match self.resolve(session_id) {
            ExecutorSelection::Local => Placement::Local,
            ExecutorSelection::Executor(name) => Placement::Hand(HandRoute {
                host: self.executors.get(&name).cloned(),
                name,
            }),
        }
    }

    fn attached(&self) -> String {
        if self.executors.is_empty() {
            return "no hands are attached".into();
        }
        let names = self
            .executors
            .keys()
            .map(|name| &**name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("attached hands: {names}")
    }

    fn forget_executor(&mut self, name: &str) {
        for places in self.sessions.values_mut() {
            places.touched.retain(|touched| &**touched != name);
            places
                .cells
                .retain(|(_, place)| place.as_deref() != Some(name));
        }
    }
}

fn disconnected(name: &str) -> CodeModeHostError {
    CodeModeHostError::new(format!(
        "hand `{name}` selected for this session is not connected; nothing was dispatched"
    ))
}

pub const PLACEMENT_INSTRUCTIONS: &str = "Code Mode scripts run on one machine: the hand that sent the prompt, the only attached hand, or the session's configured executor. Inside a script any tool call runs on another attached hand when its arguments include hand: \"<name>\", so one script can work on several machines at once, for example with Promise.all; tools.hands() lists them. Relative paths resolve from each hand's working directory; query pwd through the shell when an absolute path is needed.";

const HANDS_DESCRIPTION: &str = "List the hands attached to this session: whole machines, each with its own files, processes and tools. `hands` names them, `here` names the hand this script runs on, or is null on the brain's own machine, and `about` describes each: its platform, shell and OS version, working directory, tools, and capabilities such as a phone's photos or a browser's location, each with a `permission` of granted, prompt or denied. Inside exec, any tool call runs on another hand when its arguments include `hand: \"<name>\"`, for example `await Promise.all(names.map(hand => tools.exec_command({ cmd: \"uptime\", hand })))`; a freeform tool takes `{ input, hand }`. Calls without `hand` run where the script runs. Naming a hand that is not attached fails without running anything, and a hand lost mid-call leaves that call's outcome unknown. A capability whose permission is prompt needs the hand's user to allow it: `tools.hands({ request: { hand, capability, reason } })` asks them, with `reason` shown to them, and answers the new permission; a capability's tools may also ask on their first call. A hand whose `about` says `tools_only`, such as a browser tab or a phone, runs no scripts: call its tools with `hand` from a script that runs elsewhere.";

struct ExecutorCalls(Weak<Atom<State>>);

impl HandCalls for ExecutorCalls {
    fn here(&self, _hand: &str) -> bool {
        false
    }

    fn call<'a>(
        &'a self,
        hand: &'a str,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, ToolOutput> {
        Box::pin(async move {
            match self.0.upgrade() {
                Some(state) => {
                    CodeModeExecutors {
                        state,
                        changed: Arc::default(),
                    }
                    .call_on(hand, name, input, context)
                    .await
                }
                None => ToolOutput::error("this session's hands are gone; nothing was dispatched"),
            }
        })
    }

    fn recover<'a>(
        &'a self,
        hand: &'a str,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Option<ToolOutput>> {
        Box::pin(async move {
            let state = self.0.upgrade()?;
            let host = state.read(|state| state.executors.get(hand).cloned())?;
            host.recover_call(context).await.ok().flatten()
        })
    }
}

pub struct HandsTool(CodeModeExecutors);

impl Tool for HandsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            "hands",
            HANDS_DESCRIPTION,
            json!({
                "type": "object",
                "properties": {
                    "request": {
                        "type": "object",
                        "description": "Ask a hand's user to allow one of its capabilities",
                        "properties": {
                            "hand": {"type": "string"},
                            "capability": {"type": "string"},
                            "reason": {"type": "string", "description": "Why the task needs it, shown to the user"}
                        },
                        "required": ["hand", "capability", "reason"],
                        "additionalProperties": false
                    }
                },
                "additionalProperties": false
            }),
        )
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> HostFuture<'async_trait, ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        let here = match self.0.selection(context.session_id()) {
            ExecutorSelection::Executor(name) => Value::String(name.to_string()),
            ExecutorSelection::Local => Value::Null,
        };
        let executors = self.0.state.read(|state| state.executors.clone());
        let input = match &input {
            ToolInput::Function(raw) => serde_json::from_str::<Value>(raw.get()).ok(),
            ToolInput::Freeform(text) => serde_json::from_str::<Value>(text).ok(),
        }
        .unwrap_or(Value::Null);
        Box::pin(async move {
            if let Some(request) = input.get("request") {
                let field =
                    |key: &str| request.get(key).and_then(Value::as_str).unwrap_or_default();
                let (hand, capability, reason) =
                    (field("hand"), field("capability"), field("reason"));
                let host = executors.get(hand).ok_or_else(|| {
                    format!("hand `{hand}` is not attached; nothing was requested")
                })?;
                let permission = host
                    .request_capability(capability, reason)
                    .await
                    .map_err(|issue| issue.to_string())?;
                let result =
                    json!({"hand": hand, "capability": capability, "permission": permission});
                return Ok(ToolOutput::json(&result).with_structured_result(result));
            }
            let described =
                futures_util::future::join_all(executors.iter().map(|(name, host)| async move {
                    let mut description = host.describe();
                    if let Some(description) = &mut description
                        && !description.capabilities.is_empty()
                        && let Ok(capabilities) = host.capabilities().await
                    {
                        description.capabilities = capabilities;
                    }
                    (name.to_string(), description)
                }))
                .await;
            let names = described
                .iter()
                .map(|(name, _)| Value::String(name.clone()))
                .collect::<Vec<_>>();
            let about = described
                .into_iter()
                .filter_map(|(name, description)| {
                    Some((name, serde_json::to_value(description?).ok()?))
                })
                .collect::<serde_json::Map<_, _>>();
            let result = if about.is_empty() {
                json!({"hands": names, "here": here})
            } else {
                json!({"hands": names, "here": here, "about": about})
            };
            Ok(ToolOutput::json(&result).with_structured_result(result))
        })
    }
}

impl CodeModeExecutors {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &self,
        name: impl Into<Arc<str>>,
        host: Arc<dyn CodeModeHost>,
    ) -> Option<Arc<dyn CodeModeHost>> {
        let name = name.into();
        host.route_hands(&name, self.calls());
        let previous = self.state.update(|state| {
            let previous = state.executors.insert(Arc::clone(&name), Arc::clone(&host));
            if previous.is_some() {
                state.forget_executor(&name);
            }
            previous
        });
        self.changed.notify_waiters();
        previous
    }

    pub fn await_replacement(&self, awaiting: bool) {
        self.state.update(|state| state.awaiting = awaiting);
        self.changed.notify_waiters();
    }

    #[must_use]
    pub fn awaiting_replacement(&self) -> bool {
        self.state.read(|state| {
            state.awaiting
                && state
                    .default
                    .as_ref()
                    .is_none_or(|name| !state.executors.contains_key(name))
        })
    }

    pub async fn replacement(&self) -> Option<(Arc<str>, Arc<dyn CodeModeHost>)> {
        loop {
            let notified = self.changed.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            let (awaiting, connected) = self.state.read(|state| {
                (
                    state.awaiting,
                    state.default.as_ref().and_then(|name| {
                        state
                            .executors
                            .get(name)
                            .map(|host| (Arc::clone(name), Arc::clone(host)))
                    }),
                )
            });
            if connected.is_some() || !awaiting {
                return connected;
            }
            notified.await;
        }
    }

    pub async fn ready_placement(&self, session_id: &str) -> Placement {
        loop {
            let notified = self.changed.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            if !self
                .state
                .read(|state| state.awaits_replacement(session_id))
            {
                return self.placement(session_id);
            }
            notified.await;
        }
    }

    pub fn remove(&self, name: &str) -> Option<Arc<dyn CodeModeHost>> {
        self.state.update(|state| {
            let removed = state.executors.remove(name);
            if removed.is_some() {
                state.forget_executor(name);
            }
            removed
        })
    }

    pub fn remove_host(&self, name: &str, host: &Arc<dyn CodeModeHost>) -> bool {
        self.state
            .try_update(|state| {
                state
                    .executors
                    .get(name)
                    .is_some_and(|current| Arc::ptr_eq(current, host))
                    .then(|| {
                        state.executors.remove(name);
                        state.forget_executor(name);
                    })
            })
            .is_some()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn CodeModeHost>> {
        self.state.read(|state| state.executors.get(name).cloned())
    }

    pub fn connected(&self, name: &str) -> Result<Arc<dyn CodeModeHost>, CodeModeHostError> {
        self.get(name).ok_or_else(|| disconnected(name))
    }

    #[must_use]
    pub fn names(&self) -> Vec<Arc<str>> {
        self.state
            .read(|state| state.executors.keys().cloned().collect())
    }

    #[must_use]
    pub fn calls(&self) -> Arc<dyn HandCalls> {
        Arc::new(ExecutorCalls(Arc::downgrade(&self.state)))
    }

    #[must_use]
    pub fn tool(&self) -> HandsTool {
        HandsTool(self.clone())
    }

    pub async fn call_on(
        &self,
        hand: &str,
        name: &str,
        input: Value,
        context: ToolContext<'_>,
    ) -> ToolOutput {
        let session_id = context.session_id();
        let host = self.state.modify(|state| {
            let Some((hand, host)) = state
                .executors
                .get_key_value(hand)
                .map(|(hand, host)| (Arc::clone(hand), Arc::clone(host)))
            else {
                return Err(state.attached());
            };
            let places = state.sessions.entry(session_id.into()).or_default();
            if !places.touched.contains(&hand) {
                places.touched.push(hand);
            }
            Ok(host)
        });
        let host = match host {
            Ok(host) => host,
            Err(attached) => {
                return ToolOutput::error(format!(
                    "hand `{hand}` is not attached ({attached}); nothing was dispatched"
                ));
            }
        };
        if let Some(backend) = self
            .state
            .read(|state| state.backends.get(session_id).cloned())
        {
            host.bind_session(session_id, &backend);
        }
        match host.call(name, input, context).await {
            Ok(output) => output,
            Err(error) => ToolOutput::error(format!("hand `{hand}`: {error}")),
        }
    }

    pub fn select(&self, session_id: &str, selection: Option<ExecutorSelection>) {
        self.state.update(|state| match &selection {
            Some(selection) => {
                state
                    .selections
                    .insert(session_id.into(), selection.clone());
            }
            None => {
                state.selections.remove(session_id);
            }
        });
    }

    pub fn select_prompt_hand(&self, session_id: &str, sender: Option<&str>) {
        self.state.update(|state| {
            let runs = |name: &str| {
                state
                    .executors
                    .get(name)
                    .is_none_or(|host| host.runs_cells())
            };
            let hand = sender
                .filter(|sender| runs(sender))
                .map(Arc::from)
                .or_else(|| {
                    let mut cells = state
                        .executors
                        .keys()
                        .filter(|name| runs(name) && state.fallback.as_deref() != Some(&***name));
                    match (state.default.is_none(), cells.next(), cells.next()) {
                        (true, Some(only), None) => Some(Arc::clone(only)),
                        _ => None,
                    }
                });
            match hand {
                Some(hand) => {
                    state
                        .selections
                        .insert(session_id.into(), ExecutorSelection::Executor(hand));
                }
                None => {
                    state.selections.remove(session_id);
                }
            }
        });
    }

    #[must_use]
    pub fn selection(&self, session_id: &str) -> ExecutorSelection {
        self.state.read(|state| state.resolve(session_id))
    }

    #[must_use]
    pub fn placement(&self, session_id: &str) -> Placement {
        let (placement, backend) = self.state.read(|state| {
            (
                state.placement(session_id),
                state.backends.get(session_id).cloned(),
            )
        });
        if let Placement::Hand(HandRoute {
            host: Some(host), ..
        }) = &placement
            && let Some(backend) = backend
        {
            host.bind_session(session_id, &backend);
        }
        placement
    }

    pub fn set_default(&self, name: Option<Arc<str>>) {
        self.state.update(|state| state.default.clone_from(&name));
        self.changed.notify_waiters();
    }

    pub fn set_fallback(&self, name: Option<Arc<str>>) {
        self.state.update(|state| state.fallback.clone_from(&name));
        self.changed.notify_waiters();
    }

    #[must_use]
    pub fn default_executor(&self) -> Option<Arc<str>> {
        self.state.read(|state| state.default.clone())
    }

    pub fn clear_selections(&self, name: &str) {
        self.state.update(|state| {
            state.selections.retain(|_, selection| match selection {
                ExecutorSelection::Executor(selected) => &**selected != name,
                ExecutorSelection::Local => true,
            });
            if state.default.as_deref() == Some(name) {
                state.default = None;
            }
            if state.fallback.as_deref() == Some(name) {
                state.fallback = None;
            }
        });
    }

    pub fn link(&self, session_id: &str, root_session_id: &str) {
        if session_id != root_session_id {
            self.state.update(|state| {
                state
                    .roots
                    .insert(session_id.into(), root_session_id.into());
            });
        }
    }

    pub fn forget(&self, session_id: &str) {
        self.state.update(|state| {
            state.selections.remove(session_id);
            state.roots.remove(session_id);
            state.backends.remove(session_id);
            state.sessions.remove(session_id);
        });
    }

    pub fn record(
        &self,
        session_id: &str,
        executor: Option<&Arc<str>>,
        execution: &CodeModeExecution,
    ) {
        self.state.update(|state| {
            let places = state.sessions.entry(session_id.into()).or_default();
            if let Some(name) = executor
                && !places.touched.contains(name)
            {
                places.touched.push(Arc::clone(name));
            }
            if let Some(cell) = &execution.cell {
                places.cells.retain(|(origin, place)| {
                    **origin != *cell.origin_call_id || place.as_ref() != executor
                });
                if cell.running {
                    places
                        .cells
                        .push((cell.origin_call_id.as_str().into(), executor.cloned()));
                }
            }
        });
    }

    #[must_use]
    pub fn wait_route(&self, session_id: &str) -> Vec<Option<Arc<str>>> {
        let state = self.state.get();
        let mut places = Vec::new();
        if let Some(session) = state.sessions.get(session_id) {
            for (_, place) in session.cells.iter().rev() {
                if !places.contains(place) {
                    places.push(place.clone());
                }
            }
        }
        let current = match state.placement(session_id) {
            Placement::Local => None,
            Placement::Hand(hand) => Some(hand.name),
        };
        if !places.contains(&current) {
            places.push(current);
        }
        if state.fallback.is_some() && !places.contains(&state.fallback) {
            places.push(state.fallback.clone());
        }
        places
    }

    #[must_use]
    pub const fn missing_cell(execution: &CodeModeExecution) -> bool {
        !execution.success && execution.cell.is_none()
    }

    #[must_use]
    pub fn failed(error: &CodeModeHostError) -> CodeModeExecution {
        CodeModeExecution {
            cell: None,
            output: crate::ToolOutputBody::Text(format!("Script failed\nOutput:\n{error}")),
            success: false,
            nested_calls: Vec::new(),
            notifications: Vec::new(),
        }
    }

    fn unselected(&self) -> CodeModeHostError {
        CodeModeHostError::new(format!(
            "no hand is selected to run this session's scripts ({}); nothing was dispatched",
            self.state.read(State::attached)
        ))
    }

    #[must_use]
    pub fn touched(&self, session_id: &str) -> Vec<Arc<dyn CodeModeHost>> {
        let state = self.state.get();
        state
            .sessions
            .get(session_id)
            .map(|session| {
                session
                    .touched
                    .iter()
                    .filter_map(|name| state.executors.get(name).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn stoppable(&self, session_id: &str) -> Vec<Arc<dyn CodeModeHost>> {
        let mut hosts = self.touched(session_id);
        if let Placement::Hand(HandRoute {
            host: Some(host), ..
        }) = self.placement(session_id)
            && !hosts.iter().any(|touched| Arc::ptr_eq(touched, &host))
        {
            hosts.push(host);
        }
        hosts
    }

    pub fn settle_cells(&self, session_id: &str) {
        self.state.update(|state| {
            if let Some(session) = state.sessions.get_mut(session_id) {
                session.cells.clear();
            }
        });
    }

    pub async fn execute_on(
        &self,
        hand: HandRoute,
        tools: Vec<Arc<dyn Tool>>,
        source: &str,
        context: ToolContext<'_>,
        observer: Option<&mut dyn CodeModeObserver>,
    ) -> CodeModeExecution {
        let Some(host) = hand.host else {
            return Self::failed(&disconnected(&hand.name));
        };
        let observer = observer.map(|observer| &mut *observer as &mut dyn CodeModeObserver);
        match host
            .execute_with_local_tools(source, context, tools, observer)
            .await
        {
            Ok(execution) => {
                self.record(context.session_id(), Some(&hand.name), &execution);
                execution
            }
            Err(error) => Self::failed(&error),
        }
    }

    async fn execute_routed(
        &self,
        source: &str,
        context: ToolContext<'_>,
        tools: Vec<Arc<dyn Tool>>,
        observer: Option<&mut dyn CodeModeObserver>,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        Ok(match self.ready_placement(context.session_id()).await {
            Placement::Local => Self::failed(&self.unselected()),
            Placement::Hand(hand) => {
                self.execute_on(hand, tools, source, context, observer)
                    .await
            }
        })
    }

    async fn wait_routed(
        &self,
        input: &str,
        context: ToolContext<'_>,
        observer: &mut dyn CodeModeObserver,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        let session_id = context.session_id();
        let places = self.wait_route(session_id);
        let remaining = places.len();
        let mut failure = self.unselected();
        let mut missing = None;
        for (index, place) in places.into_iter().enumerate() {
            let Some(name) = place else {
                continue;
            };
            let host = match self.connected(&name) {
                Ok(host) => host,
                Err(error) => {
                    failure = error;
                    continue;
                }
            };
            match host
                .wait_with_updates(input, context, &mut *observer as &mut dyn CodeModeObserver)
                .await
            {
                Ok(execution) if Self::missing_cell(&execution) && index + 1 < remaining => {
                    missing = Some(execution);
                }
                Ok(execution) => {
                    self.record(session_id, Some(&name), &execution);
                    return Ok(execution);
                }
                Err(error) => failure = error,
            }
        }
        Ok(missing.unwrap_or_else(|| Self::failed(&failure)))
    }
}

impl CodeModeHost for CodeModeExecutors {
    fn tool_mode(&self) -> EmbeddedToolMode {
        EmbeddedToolMode::CodeOnly
    }

    fn supports_cells(&self) -> bool {
        true
    }

    fn tool_definitions(&self, session_id: &str) -> Result<Vec<ToolDefinition>, CodeModeHostError> {
        match self.placement(session_id) {
            Placement::Hand(HandRoute {
                host: Some(host), ..
            }) => host.tool_definitions(session_id),
            _ => Ok(Vec::new()),
        }
    }

    fn execute<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(self.execute_routed(source, context, Vec::new(), None))
    }

    fn execute_with_updates<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(self.execute_routed(source, context, Vec::new(), Some(observer)))
    }

    fn execute_with_local_tools<'a>(
        &'a self,
        source: &'a str,
        context: ToolContext<'a>,
        tools: Vec<Arc<dyn Tool>>,
        observer: Option<&'a mut dyn CodeModeObserver>,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(self.execute_routed(source, context, tools, observer))
    }

    fn wait_with_updates<'a>(
        &'a self,
        input: &'a str,
        context: ToolContext<'a>,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<CodeModeExecution, CodeModeHostError>> {
        Box::pin(self.wait_routed(input, context, observer))
    }

    fn recover<'a>(
        &'a self,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Option<CodeModeExecution>, CodeModeHostError>> {
        Box::pin(async move {
            let mut routes = Vec::new();
            if let Placement::Hand(HandRoute {
                name,
                host: Some(host),
            }) = self.placement(context.session_id())
            {
                routes.push((name, host));
            }
            if let Some((name, host)) = self.state.read(|state| {
                let name = state.fallback.clone()?;
                let host = state.executors.get(&name).cloned()?;
                Some((name, host))
            }) && !routes.iter().any(|(placed, _)| *placed == name)
            {
                routes.push((name, host));
            }
            for (name, host) in routes {
                let recovered = host.recover(context).await?;
                if let Some(execution) = &recovered {
                    self.record(context.session_id(), Some(&name), execution);
                    return Ok(recovered);
                }
            }
            Ok(None)
        })
    }

    fn adopt<'a>(
        &'a self,
        context: ToolContext<'a>,
        tools: Vec<Arc<dyn Tool>>,
    ) -> HostFuture<'a, ()> {
        Box::pin(async move {
            let hosts = self.state.read(|state| {
                state
                    .executors
                    .values()
                    .filter(|host| host.runs_cells())
                    .cloned()
                    .collect::<Vec<_>>()
            });
            for host in hosts {
                host.adopt(context, tools.clone()).await;
            }
        })
    }

    fn bind_session(&self, session_id: &str, backend: &str) {
        self.state.update(|state| {
            state.backends.insert(session_id.into(), backend.into());
        });
    }
    fn link_session(&self, session_id: &str, root_session_id: &str) {
        self.link(session_id, root_session_id);
    }

    fn begin_turn(&self, session_id: &str) {
        for host in self.touched(session_id) {
            host.begin_turn(session_id);
        }
    }

    fn detach_turn(
        &self,
        session_id: &str,
        observer: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) {
        for host in self.touched(session_id) {
            host.detach_turn(session_id, observer);
        }
    }

    fn preempt_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            let mut outcome = Ok(());
            for host in self.touched(session_id) {
                let result = host.preempt_turn(session_id).await;
                outcome = outcome.and(result);
            }
            outcome
        })
    }

    fn settle_preempt(&self, session_id: &str) {
        for host in self.touched(session_id) {
            host.settle_preempt(session_id);
        }
    }

    fn cancel_turn<'a>(
        &'a self,
        session_id: &'a str,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            let mut outcome = Ok(());
            for host in self.stoppable(session_id) {
                let result = host.cancel_turn(session_id).await;
                outcome = outcome.and(result);
            }
            self.settle_cells(session_id);
            outcome
        })
    }

    fn cancel_turn_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            let mut outcome = Ok(());
            for host in self.stoppable(session_id) {
                let result = host.cancel_turn_with_updates(session_id, observer).await;
                outcome = outcome.and(result);
            }
            self.settle_cells(session_id);
            outcome
        })
    }

    fn cancel_with_updates<'a>(
        &'a self,
        session_id: &'a str,
        observer: &'a mut dyn CodeModeObserver,
    ) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            let mut outcome = Ok(());
            for host in self.stoppable(session_id) {
                let result = host.cancel_with_updates(session_id, observer).await;
                outcome = outcome.and(result);
            }
            self.settle_cells(session_id);
            outcome
        })
    }

    fn cancel<'a>(&'a self, session_id: &'a str) -> HostFuture<'a, Result<(), CodeModeHostError>> {
        Box::pin(async move {
            let mut outcome = Ok(());
            for host in self.stoppable(session_id) {
                let result = host.cancel(session_id).await;
                outcome = outcome.and(result);
            }
            self.settle_cells(session_id);
            outcome
        })
    }
}
