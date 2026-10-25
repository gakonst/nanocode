use super::*;
use crate::embedded::{CodeModeHost, CodeModeHostError};
use nanocode_code_mode::remote::{CodeModeExecutors, HandRoute, Placement};
use nanocode_sync::Atom;

pub struct ToolRuntime {
    inner: Runtime,
    recovery: Option<Arc<dyn crate::ToolRecovery>>,
}

enum Runtime {
    Native(NativeToolRuntime),
    Host(crate::embedded::EmbeddedToolRuntime),
}

#[derive(Clone)]
pub struct ToolRuntimeControl {
    inner: RuntimeControl,
}

#[derive(Clone)]
enum RuntimeControl {
    Native(NativeToolRuntimeControl),
    Host(crate::embedded::EmbeddedToolRuntimeControl),
}

impl ToolRuntime {
    pub fn new(workspace: impl Into<PathBuf>, builtin_tools: Vec<Arc<dyn Tool>>) -> Self {
        Self {
            inner: Runtime::Native(NativeToolRuntime::new(workspace, builtin_tools)),
            recovery: None,
        }
    }

    #[must_use]
    pub fn new_with_tools(
        workspace: impl Into<PathBuf>,
        builtin_tools: Vec<Arc<dyn Tool>>,
        tools: &Tools,
    ) -> Self {
        let inner = if tools.embedded_host.is_some() {
            Runtime::Host(crate::embedded::EmbeddedToolRuntime::new_with_tools(
                workspace,
                builtin_tools,
                tools,
            ))
        } else {
            Runtime::Native(NativeToolRuntime::new_with_tools(
                workspace,
                builtin_tools,
                tools,
            ))
        };
        Self {
            inner,
            recovery: tools.recovery(),
        }
    }

    #[must_use]
    pub fn control(&self) -> ToolRuntimeControl {
        ToolRuntimeControl {
            inner: match &self.inner {
                Runtime::Native(runtime) => RuntimeControl::Native(runtime.control()),
                Runtime::Host(runtime) => RuntimeControl::Host(runtime.control()),
            },
        }
    }

    pub fn default_shell_name(&self) -> &str {
        match &self.inner {
            Runtime::Native(runtime) => runtime.default_shell_name(),
            Runtime::Host(runtime) => runtime.default_shell_name(),
        }
    }

    pub fn working_directory(&self) -> &str {
        match &self.inner {
            Runtime::Native(runtime) => runtime.working_directory(),
            Runtime::Host(runtime) => runtime.working_directory(),
        }
    }

    pub fn is_code_only(&self) -> bool {
        match &self.inner {
            Runtime::Native(runtime) => runtime.is_code_only(),
            Runtime::Host(runtime) => runtime.is_code_only(),
        }
    }

    #[must_use]
    pub fn running_tasks(&self) -> Vec<crate::contract::RunningTask> {
        match &self.inner {
            Runtime::Native(runtime) => runtime
                .workspace
                .iter()
                .flat_map(|workspace| workspace.tools())
                .flat_map(|tool| tool.running_tasks())
                .collect(),
            Runtime::Host(_) => Vec::new(),
        }
    }

    pub fn model_specs(&self, session_id: &str) -> Vec<ToolDefinition> {
        match &self.inner {
            Runtime::Native(runtime) => runtime.model_specs(session_id),
            Runtime::Host(runtime) => runtime.model_specs(session_id),
        }
    }

    pub(crate) fn model_contract(
        &self,
        session_id: &str,
    ) -> (Vec<ToolDefinition>, Vec<(String, String)>) {
        match &self.inner {
            Runtime::Native(runtime) => runtime.model_contract(session_id),
            Runtime::Host(runtime) => runtime.model_contract(session_id),
        }
    }

    #[doc(hidden)]
    pub fn registered_code_mode_definitions(&self) -> Vec<ToolDefinition> {
        match &self.inner {
            Runtime::Native(runtime) => runtime.registry.registered_code_mode_definitions(),
            Runtime::Host(runtime) => runtime.registered_code_mode_definitions(),
        }
    }

    pub fn supports_parallel_tool_calls(&self, name: &str) -> bool {
        match &self.inner {
            Runtime::Native(runtime) => runtime.supports_parallel_tool_calls(name),
            Runtime::Host(runtime) => runtime.supports_parallel_tool_calls(name),
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        match &self.inner {
            Runtime::Native(runtime) => runtime.contains(name),
            Runtime::Host(runtime) => runtime.contains(name),
        }
    }

    pub fn execute_code<'call>(
        &'call self,
        source: &'call str,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(runtime) => runtime.execute_code(source, context).await,
                Runtime::Host(runtime) => runtime.execute_code(source, context).await,
            }
        })
    }

    pub fn execute_code_with_updates<'call>(
        &'call self,
        source: &'call str,
        context: ToolContext<'call>,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(runtime) => {
                    runtime
                        .execute_code_with_updates(source, context, observer)
                        .await
                }
                Runtime::Host(runtime) => {
                    runtime
                        .execute_code_with_updates(source, context, observer)
                        .await
                }
            }
        })
    }

    pub fn execute_code_owned<'call>(
        &'call self,
        source: &'call str,
        context: OwnedToolContext,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(runtime) => runtime.execute_code_owned(source, context).await,
                Runtime::Host(runtime) => runtime.execute_code_owned(source, context).await,
            }
        })
    }

    pub fn execute_code_owned_with_updates<'call>(
        &'call self,
        source: &'call str,
        context: OwnedToolContext,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(runtime) => {
                    runtime
                        .execute_code_owned_with_updates(source, context, observer)
                        .await
                }
                Runtime::Host(runtime) => {
                    runtime
                        .execute_code_owned_with_updates(source, context, observer)
                        .await
                }
            }
        })
    }

    pub fn wait_for_code<'call>(
        &'call self,
        input: &'call str,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(runtime) => runtime.wait_for_code(input, context).await,
                Runtime::Host(runtime) => runtime.wait_for_code(input, context).await,
            }
        })
    }

    pub fn wait_for_code_with_updates<'call>(
        &'call self,
        input: &'call str,
        context: ToolContext<'call>,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(runtime) => {
                    runtime
                        .wait_for_code_with_updates(input, context, observer)
                        .await
                }
                Runtime::Host(runtime) => {
                    runtime
                        .wait_for_code_with_updates(input, context, observer)
                        .await
                }
            }
        })
    }

    pub fn recover_code<'call>(
        &'call self,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<Option<CodeModeExecution>, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(runtime) => runtime.recover_code(context).await,
                Runtime::Host(runtime) => runtime.recover_code(context).await,
            }
        })
    }

    pub fn recover_tool<'call>(
        &'call self,
        name: &'call str,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<'call, Option<ToolOutput>> {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(_) => self.recovery.as_ref()?.recover(name, context).await,
                Runtime::Host(runtime) => runtime.recover_tool(name, context).await,
            }
        })
    }

    pub fn execute_tool<'call>(
        &'call self,
        name: &'call str,
        input: ToolInput,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<ToolOutput, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match &self.inner {
                Runtime::Native(runtime) => runtime.execute_tool(name, input, context).await,
                Runtime::Host(runtime) => runtime.execute_tool(name, input, context).await,
            }
        })
    }

    #[doc(hidden)]
    pub fn execute_nested_tool<'call>(
        &'call self,
        name: &'call str,
        input: Value,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<'call, ToolOutput> {
        match &self.inner {
            Runtime::Native(runtime) => runtime.execute_nested_tool(name, input, context),
            Runtime::Host(_) => Box::pin(async move {
                ToolOutput::error(format!(
                    "`{name}` cannot run here for another hand because this runtime delegates Code Mode to its own host"
                ))
            }),
        }
    }
}

impl ToolRuntimeControl {
    pub fn settle_preempt(&self) {
        match &self.inner {
            RuntimeControl::Native(control) => control.settle_preempt(),
            RuntimeControl::Host(control) => control.settle_preempt(),
        }
    }

    pub fn preempt_turn<'call>(&'call self) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            match &self.inner {
                RuntimeControl::Native(control) => control.preempt_turn().await,
                RuntimeControl::Host(control) => control.preempt_turn().await,
            }
        })
    }

    pub fn begin_turn(&self) {
        match &self.inner {
            RuntimeControl::Native(control) => control.begin_turn(),
            RuntimeControl::Host(control) => control.begin_turn(),
        }
    }

    pub fn detach_turn_with_updates<'call>(
        &'call self,
        observer: &'call mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            match &self.inner {
                RuntimeControl::Native(control) => control.detach_turn_with_updates(observer).await,
                RuntimeControl::Host(control) => control.detach_turn_with_updates(observer).await,
            }
        })
    }

    pub fn cancel_turn<'call>(&'call self) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            match &self.inner {
                RuntimeControl::Native(control) => control.cancel_turn().await,
                RuntimeControl::Host(control) => control.cancel_turn().await,
            }
        })
    }

    pub fn cancel_turn_with_updates<'call>(
        &'call self,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            match &self.inner {
                RuntimeControl::Native(control) => control.cancel_turn_with_updates(observer).await,
                RuntimeControl::Host(control) => control.cancel_turn_with_updates(observer).await,
            }
        })
    }

    pub fn cancel<'call>(&'call self) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            match &self.inner {
                RuntimeControl::Native(control) => control.cancel().await,
                RuntimeControl::Host(control) => control.cancel().await,
            }
        })
    }

    pub fn cancel_with_updates<'call>(
        &'call self,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            match &self.inner {
                RuntimeControl::Native(control) => control.cancel_with_updates(observer).await,
                RuntimeControl::Host(control) => control.cancel_with_updates(observer).await,
            }
        })
    }
}

struct NativeToolRuntime {
    pub(super) registry: Arc<ToolRegistry>,
    exposure: Option<ToolExposure>,
    deferred_tools_guidance_enabled: bool,
    code_mode: code_mode::CodeModeRuntime,
    workspace: Option<Arc<dyn crate::workspace_runtime::WorkspaceExecutor>>,
    current_turn: Arc<AtomicU64>,
    default_shell_name: Arc<str>,
    working_directory: Arc<str>,
    executors: Option<CodeModeExecutors>,
    sessions: Arc<Atom<HashSet<Arc<str>>>>,
}

#[doc(hidden)]
#[derive(Clone)]
struct NativeToolRuntimeControl {
    code_mode: code_mode::CodeModeControl,
    workspace: Option<Arc<dyn crate::workspace_runtime::WorkspaceExecutor>>,
    current_turn: Arc<AtomicU64>,
    executors: Option<CodeModeExecutors>,
    sessions: Arc<Atom<HashSet<Arc<str>>>>,
}

struct IgnoreUpdates;

impl CodeModeObserver for IgnoreUpdates {
    fn update(&mut self, _update: crate::embedded::CodeModeUpdate<'_>) {}
}

impl NativeToolRuntime {
    pub fn new(workspace: impl Into<PathBuf>, builtin_tools: Vec<Arc<dyn Tool>>) -> Self {
        Self::new_inner(workspace.into(), builtin_tools, None)
    }

    pub fn new_with_tools(
        workspace: impl Into<PathBuf>,
        builtin_tools: Vec<Arc<dyn Tool>>,
        tools: &Tools,
    ) -> Self {
        let source = tools
            .workspace_source()
            .filter(|_| tools.workspace_enabled());
        let root = source.map_or_else(|| workspace.into(), |source| source.root().to_owned());
        let executor =
            source.map(|source| source.create(tools.process_environment().as_ref().clone(), None));
        Self::new_inner(root, builtin_tools, executor).with_tools(tools)
    }

    fn new_inner(
        workspace: PathBuf,
        builtin_tools: Vec<Arc<dyn Tool>>,
        executor: Option<Arc<dyn crate::workspace_runtime::WorkspaceExecutor>>,
    ) -> Self {
        let current_turn = Arc::new(AtomicU64::new(0));
        let default_shell_name = Arc::from(nanocode_home::shell::default_shell_name());
        let working_directory = Arc::from(workspace.to_string_lossy().into_owned());
        let mut handlers = executor
            .as_ref()
            .map_or_else(Vec::new, |workspace| workspace.tools());
        handlers.extend(builtin_tools);
        Self {
            registry: Arc::new(ToolRegistry::from_ordered(handlers)),
            exposure: None,
            deferred_tools_guidance_enabled: false,
            code_mode: code_mode::CodeModeRuntime::new_with_turn(Arc::clone(&current_turn)),
            workspace: executor,
            current_turn,
            default_shell_name,
            working_directory,
            executors: None,
            sessions: Arc::default(),
        }
    }

    fn with_tools(mut self, tools: &Tools) -> Self {
        tools.start_providers();
        let registry = Arc::make_mut(&mut self.registry);
        registry
            .excluded_code_tools
            .clone_from(&tools.excluded_code_tools);
        for policy in tools.code_mode_policies() {
            if !registry
                .policies
                .iter()
                .any(|existing| Arc::ptr_eq(existing, &policy))
            {
                registry.policies.push(policy);
            }
        }
        if self.exposure.is_none() {
            let exposure = tools.exposure();
            self.exposure = Some(exposure);
            registry.set_all_exposures(exposure);
        }
        self.deferred_tools_guidance_enabled |= tools.deferred_tools_guidance_enabled;
        registry.extend(tools.registered.iter().map(|tool| {
            (
                Arc::clone(&tool.handler),
                tool.exposure.unwrap_or_else(|| tools.exposure()),
            )
        }));
        registry.extend(
            tools
                .provider_direct
                .iter()
                .cloned()
                .map(|tool| (tool, tools.exposure())),
        );
        registry.providers.extend(tools.providers.iter().cloned());
        registry.route_workspaces(
            &tools.remote_runners,
            tools.process_route_store.clone(),
            tools.runner_pool.as_ref(),
        );
        if let Some(working_directory) = &tools.working_directory {
            self.working_directory = Arc::clone(working_directory);
        }
        if let Some(default_shell) = &tools.default_shell {
            self.default_shell_name = Arc::clone(default_shell);
        }
        self.executors.clone_from(&tools.code_mode_executors);
        registry.hand_calls = tools.hand_calls.clone().or_else(|| {
            tools
                .code_mode_executors
                .as_ref()
                .map(CodeModeExecutors::calls)
        });
        self
    }

    fn track(&self, session_id: &str) {
        if !self.sessions.read(|sessions| sessions.contains(session_id)) {
            self.sessions.update(|sessions| {
                if !sessions.contains(session_id) {
                    sessions.insert(Arc::from(session_id));
                }
            });
        }
    }

    fn placement(&self, session_id: &str) -> Placement {
        let Some(executors) = &self.executors else {
            return Placement::Local;
        };
        self.track(session_id);
        executors.placement(session_id)
    }

    fn record(&self, session_id: &str, executor: Option<&Arc<str>>, execution: &CodeModeExecution) {
        if let Some(executors) = &self.executors {
            executors.record(session_id, executor, execution);
        }
    }

    fn callbacks(
        &self,
        host: Option<&Arc<dyn CodeModeHost>>,
        session_id: &str,
    ) -> Vec<Arc<dyn Tool>> {
        let Some(host) = host else {
            return Vec::new();
        };
        let remote = host
            .tool_definitions(session_id)
            .unwrap_or_default()
            .iter()
            .map(|definition| definition.name().to_owned())
            .collect::<HashSet<_>>();
        self.registry.callback_tools(&remote)
    }

    async fn execute_routed(
        &self,
        source: &str,
        context: OwnedToolContext,
        observer: Option<&mut dyn CodeModeObserver>,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        if let (Some(executors), Placement::Hand(hand)) =
            (&self.executors, self.placement(&context.session_id))
        {
            let tools = self.callbacks(hand.host.as_ref(), &context.session_id);
            return Ok(executors
                .execute_on(hand, tools, source, context.as_context(), observer)
                .await);
        }
        let session_id = self.executors.is_some().then(|| context.session_id.clone());
        let execution = match observer {
            Some(observer) => {
                self.code_mode
                    .execute_with_updates(source, self.registry.clone(), context, observer)
                    .await
            }
            None => {
                self.code_mode
                    .execute(source, self.registry.clone(), context)
                    .await
            }
        };
        if let Some(session_id) = session_id {
            self.record(&session_id, None, &execution);
        }
        Ok(execution)
    }

    #[must_use]
    pub fn default_shell_name(&self) -> &str {
        &self.default_shell_name
    }

    #[must_use]
    pub fn working_directory(&self) -> &str {
        &self.working_directory
    }

    #[doc(hidden)]
    #[must_use]
    pub fn control(&self) -> NativeToolRuntimeControl {
        NativeToolRuntimeControl {
            code_mode: self.code_mode.control(),
            workspace: self.workspace.clone(),
            current_turn: Arc::clone(&self.current_turn),
            executors: self.executors.clone(),
            sessions: Arc::clone(&self.sessions),
        }
    }

    #[must_use]
    pub fn is_code_only(&self) -> bool {
        self.exposure.unwrap_or_default() == ToolExposure::CodeModeOnly
    }

    #[must_use]
    pub fn model_specs(&self, _session_id: &str) -> Vec<ToolDefinition> {
        let mut nested = self.registry.registered_code_mode_definitions();
        let provider_summaries = self.registry.code_mode_tool_summaries();
        let mut direct = self
            .registry
            .direct_definitions()
            .filter(|definition| !matches!(definition, ToolDefinition::ToolSearch { .. }))
            .cloned()
            .collect::<Vec<_>>();
        let code_mode_names = nested
            .iter()
            .map(|definition| definition.name().to_owned())
            .collect::<HashSet<_>>();
        direct = group_direct_code_mode_definitions(direct, &code_mode_names);
        crate::code_mode_order::sort_definitions(&mut nested);
        let mut native = vec![
            code_mode::exec_spec(
                &nested,
                &provider_summaries,
                self.deferred_tools_guidance_enabled,
                self.exposure.unwrap_or_default() == ToolExposure::CodeModeOnly,
                false,
            ),
            code_mode::wait_spec(),
        ];
        if self.is_code_only() {
            return native;
        }
        native.append(&mut direct);
        native.extend(
            self.registry
                .definitions()
                .iter()
                .filter(|definition| matches!(definition, ToolDefinition::ToolSearch { .. }))
                .cloned(),
        );
        native
    }

    pub(crate) fn model_contract(
        &self,
        session_id: &str,
    ) -> (Vec<ToolDefinition>, Vec<(String, String)>) {
        (
            self.model_specs(session_id),
            self.registry.code_mode_tool_names(),
        )
    }

    #[must_use]
    pub fn supports_parallel_tool_calls(&self, name: &str) -> bool {
        self.registry.supports_parallel_tool_calls(name)
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.registry.contains(name)
    }

    pub fn execute_code<'call>(
        &'call self,
        source: &'call str,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(self.execute_routed(source, OwnedToolContext::from_context(context), None))
    }

    #[doc(hidden)]
    pub fn execute_code_with_updates<'call>(
        &'call self,
        source: &'call str,
        context: ToolContext<'call>,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(self.execute_routed(
            source,
            OwnedToolContext::from_context(context),
            Some(observer),
        ))
    }

    #[doc(hidden)]
    pub fn execute_code_owned<'call>(
        &'call self,
        source: &'call str,
        context: OwnedToolContext,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(self.execute_routed(source, context, None))
    }

    #[doc(hidden)]
    pub fn execute_code_owned_with_updates<'call>(
        &'call self,
        source: &'call str,
        context: OwnedToolContext,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(self.execute_routed(source, context, Some(observer)))
    }

    pub fn wait_for_code<'call>(
        &'call self,
        input: &'call str,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            self.wait_for_code_with_updates(input, context, &mut IgnoreUpdates)
                .await
        })
    }

    #[doc(hidden)]
    pub fn wait_for_code_with_updates<'call>(
        &'call self,
        input: &'call str,
        context: ToolContext<'call>,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<CodeModeExecution, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            let Some(executors) = &self.executors else {
                return Ok(self
                    .code_mode
                    .wait_with_updates(input, context, observer)
                    .await);
            };
            let session_id = context.session_id();
            self.track(session_id);
            let places = executors.wait_route(session_id);
            let remaining = places.len();
            let mut failure = None;
            let mut missing = None;
            for (index, place) in places.into_iter().enumerate() {
                let execution = match &place {
                    None => {
                        self.code_mode
                            .wait_with_updates(input, context, &mut *observer)
                            .await
                    }
                    Some(name) => {
                        let waited = match executors.connected(name) {
                            Ok(host) => {
                                host.wait_with_updates(
                                    input,
                                    context,
                                    &mut *observer as &mut dyn CodeModeObserver,
                                )
                                .await
                            }
                            Err(error) => Err(error),
                        };
                        match waited {
                            Ok(execution) => execution,
                            Err(error) => {
                                failure = Some(error);
                                continue;
                            }
                        }
                    }
                };
                if CodeModeExecutors::missing_cell(&execution) && index + 1 < remaining {
                    missing = Some(execution);
                    continue;
                }
                executors.record(session_id, place.as_ref(), &execution);
                return Ok(execution);
            }
            Ok(match (missing, failure) {
                (Some(execution), _) => execution,
                (None, Some(error)) => CodeModeExecutors::failed(&error),
                (None, None) => CodeModeExecutors::failed(&CodeModeHostError::new(
                    "no Code Mode placement is available for this session",
                )),
            })
        })
    }

    pub fn recover_code<'call>(
        &'call self,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<Option<CodeModeExecution>, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            match (&self.executors, self.placement(context.session_id())) {
                (
                    Some(_),
                    Placement::Hand(HandRoute {
                        host: Some(host), ..
                    }),
                ) => host.recover(context).await,
                _ => Ok(None),
            }
        })
    }

    pub fn execute_tool<'call>(
        &'call self,
        name: &'call str,
        input: ToolInput,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<
        'call,
        Result<ToolOutput, crate::embedded::CodeModeHostError>,
    > {
        Box::pin(async move {
            if self.registry.workspace_tool(name)
                && let Placement::Hand(hand) = self.placement(context.session_id())
            {
                return Ok(ToolOutput::error(format!(
                    "Code Mode for this session runs on hand {}; call `tools.{name}` inside exec to run it there. No direct handler was invoked.",
                    hand.name
                )));
            }
            Ok(self.registry.execute_direct(name, input, context).await)
        })
    }

    #[doc(hidden)]
    pub fn execute_nested_tool<'call>(
        &'call self,
        name: &'call str,
        input: Value,
        context: ToolContext<'call>,
    ) -> futures_util::future::BoxFuture<'call, ToolOutput> {
        Box::pin(self.registry.execute_nested(name, input, context))
    }
}

fn group_direct_code_mode_definitions(
    definitions: Vec<ToolDefinition>,
    code_mode_names: &HashSet<String>,
) -> Vec<ToolDefinition> {
    let mut grouped = Vec::<ToolDefinition>::new();
    for definition in definitions {
        let canonical_name = definition.name().to_owned();
        let mut definition = if code_mode_names.contains(&canonical_name) {
            crate::code_mode_description::augment_definition_for_code_mode(definition)
        } else {
            definition
        };
        let Some((namespace, name)) = canonical_name.rsplit_once("__") else {
            grouped.push(definition);
            continue;
        };
        if namespace.is_empty() || name.is_empty() {
            grouped.push(definition);
            continue;
        }
        let ToolDefinition::Function {
            name: direct_name, ..
        } = &mut definition
        else {
            grouped.push(definition);
            continue;
        };
        *direct_name = name.into();
        if let Some(ToolDefinition::Namespace { tools, .. }) = grouped.iter_mut().find(
            |group| matches!(group, ToolDefinition::Namespace { name, .. } if &**name == namespace),
        ) {
            tools.push(definition);
        } else {
            grouped.push(ToolDefinition::namespace(
                namespace,
                format!("Tools in the {namespace} namespace."),
                [definition],
            ));
        }
    }
    grouped
}

impl NativeToolRuntimeControl {
    fn sessions(&self) -> Vec<Arc<str>> {
        self.sessions
            .read(|sessions| sessions.iter().cloned().collect())
    }

    fn remote(&self) -> Vec<(Arc<str>, Arc<dyn CodeModeHost>)> {
        let Some(executors) = &self.executors else {
            return Vec::new();
        };
        self.sessions()
            .into_iter()
            .flat_map(|session| {
                executors
                    .touched(&session)
                    .into_iter()
                    .map(move |host| (Arc::clone(&session), host))
            })
            .collect()
    }

    fn settle_cells(&self) {
        if let Some(executors) = &self.executors {
            for session in self.sessions() {
                executors.settle_cells(&session);
            }
        }
    }

    pub fn settle_preempt(&self) {
        self.code_mode.settle_preempt();
        for (session, host) in self.remote() {
            host.settle_preempt(&session);
        }
    }

    pub fn preempt_turn<'call>(&'call self) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            self.code_mode.preempt_turn();
            for (session, host) in self.remote() {
                if let Err(error) = host.preempt_turn(&session).await {
                    tracing::warn!(target: "nanocode_code_mode", %error, "remote Code Mode preemption failed");
                }
            }
        })
    }

    #[doc(hidden)]
    pub fn begin_turn(&self) {
        let _ = self
            .current_turn
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |turn| {
                Some(turn.saturating_add(1))
            });
        for (session, host) in self.remote() {
            host.begin_turn(&session);
        }
    }

    pub fn detach_turn_with_updates<'call>(
        &'call self,
        observer: &'call mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            let turn_id = self.current_turn.load(Ordering::Acquire);
            self.code_mode.detach_turn(turn_id, &mut *observer).await;
            for (session, host) in self.remote() {
                host.detach_turn(&session, &mut *observer);
            }
        })
    }

    #[doc(hidden)]
    pub fn cancel_turn<'call>(&'call self) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            let turn_id = self.current_turn.load(Ordering::Acquire);
            self.code_mode.terminate_turn(turn_id, None).await;
            for (session, host) in self.remote() {
                if let Err(error) = host.cancel_turn(&session).await {
                    tracing::warn!(target: "nanocode_code_mode", %error, "remote Code Mode turn cancellation failed");
                }
            }
            self.settle_cells();
        })
    }

    pub fn cancel_turn_with_updates<'call>(
        &'call self,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            let turn_id = self.current_turn.load(Ordering::Acquire);

            self.code_mode
                .terminate_turn(turn_id, Some(&mut *observer))
                .await;
            for (session, host) in self.remote() {
                if let Err(error) = host
                    .cancel_turn_with_updates(&session, &mut *observer as &mut dyn CodeModeObserver)
                    .await
                {
                    tracing::warn!(target: "nanocode_code_mode", %error, "remote Code Mode turn cancellation failed");
                }
            }
            self.settle_cells();
        })
    }

    #[doc(hidden)]
    pub fn cancel<'call>(&'call self) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            let _code_mode_quiescence = self.code_mode.terminate_all(None).await;
            if let Some(workspace) = &self.workspace {
                workspace.cancel().await;
            }
            for (session, host) in self.remote() {
                if let Err(error) = host.cancel(&session).await {
                    tracing::warn!(target: "nanocode_code_mode", %error, "remote Code Mode cancellation failed");
                }
            }
            self.settle_cells();
        })
    }

    pub fn cancel_with_updates<'call>(
        &'call self,
        observer: &'call mut dyn CodeModeObserver,
    ) -> futures_util::future::BoxFuture<'call, ()> {
        Box::pin(async move {
            let _code_mode_quiescence = self.code_mode.terminate_all(Some(&mut *observer)).await;
            if let Some(workspace) = &self.workspace {
                workspace.cancel().await;
            }
            for (session, host) in self.remote() {
                if let Err(error) = host
                    .cancel_with_updates(&session, &mut *observer as &mut dyn CodeModeObserver)
                    .await
                {
                    tracing::warn!(target: "nanocode_code_mode", %error, "remote Code Mode cancellation failed");
                }
            }
            self.settle_cells();
        })
    }
}

pub(super) fn record_tool_content(span: &tracing::Span, kind: &'static str, content: &str) {
    span.in_scope(|| {
        info!(
            target: "nanocode_code_mode",
            content_kind = kind,
            content,
            "tool content"
        );
    });
}

pub(super) fn panicked_tool_output(
    span: &tracing::Span,
    payload: Box<dyn Any + Send>,
) -> ToolOutput {
    let message = panic_payload(payload);
    record_tool_content(span, "tool.panic", &message);
    ToolOutput::error("aborted")
}

fn panic_payload(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => payload.downcast::<&'static str>().map_or_else(
            |_| "non-string panic payload".to_owned(),
            |message| (*message).to_owned(),
        ),
    }
}

pub(super) fn tool_execution_span(
    name: &str,
    context: ToolContext<'_>,
    arguments_bytes: usize,
    arguments_kind: &'static str,
    arguments_count: usize,
    argument_keys: &str,
) -> tracing::Span {
    info_span!(
        target: "nanocode_code_mode",
        "tool.execute",
        otel.kind = "internal",
        otel.status_code = tracing::field::Empty,
        tool.name = name,
        session.id = context.session_id(),
        tool.call_id = context.call_id(),
        tool.arguments.bytes = arguments_bytes,
        tool.arguments.kind = arguments_kind,
        tool.arguments.count = arguments_count,
        tool.arguments.keys = argument_keys,
        tool.admission = tracing::field::Empty,
        tool.admission.wait_ns = tracing::field::Empty,
        tool.hand = tracing::field::Empty,
        process.exit.code = tracing::field::Empty,
        process.running = tracing::field::Empty,
        process.wall_time_ms = tracing::field::Empty,
        shell.session.id = tracing::field::Empty,
        tool.output.bytes = tracing::field::Empty,
        tool.output.original_tokens = tracing::field::Empty,
        status = tracing::field::Empty,
        duration_ns = tracing::field::Empty,
    )
}

pub(super) fn finish_tool_execution_span(
    span: &tracing::Span,
    started_at: std::time::Instant,
    execution: &ToolOutput,
    output_content: Option<&str>,
) {
    if let Some(output_content) = output_content {
        record_tool_content(span, "tool.output", output_content);
        span.record("tool.output.bytes", output_content.len());
    }
    span.record(
        "status",
        if execution.success {
            "completed"
        } else {
            "failed"
        },
    );
    span.record(
        "otel.status_code",
        if execution.success { "OK" } else { "ERROR" },
    );
    span.record(
        "duration_ns",
        u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
    );
    if let Some(process) = execution.process_trace() {
        if let Some(exit_code) = process.exit_code {
            span.record("process.exit.code", exit_code);
        }
        span.record("process.running", process.session_id.is_some());
        span.record("process.wall_time_ms", process.wall_time_seconds * 1_000.0);
        if let Some(session_id) = process.session_id {
            span.record("shell.session.id", session_id);
        }
        span.record("tool.output.bytes", process.output_bytes);
        if let Some(original_token_count) = process.original_token_count {
            span.record("tool.output.original_tokens", original_token_count);
        }
    }
}
