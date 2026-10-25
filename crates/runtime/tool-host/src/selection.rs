use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

#[cfg(not(target_family = "wasm"))]
use crate::attachment::{
    AttachmentError, ProcessRouteStore, RemoteMode, RemoteRoute, RemoteRunnerCatalog, RunnerRouting,
};
use crate::{Tool, ToolDefinition};
#[cfg(not(target_family = "wasm"))]
use std::{any::Any, ffi::OsString, path::PathBuf};

#[cfg(not(target_family = "wasm"))]
pub use nanocode_tool_provider::DynamicToolProvider;
pub use nanocode_tool_provider::ToolExposure;

#[derive(Clone)]
pub(crate) struct RegisteredTool {
    pub(crate) handler: Arc<dyn Tool>,

    pub(crate) exposure: Option<ToolExposure>,
}

pub trait ToolSource: Sized {
    #[doc(hidden)]
    fn install(self, builder: ToolsBuilder) -> ToolsBuilder;
}

impl<T: Tool + 'static> ToolSource for T {
    fn install(self, builder: ToolsBuilder) -> ToolsBuilder {
        builder.tool(self)
    }
}

#[cfg(not(target_family = "wasm"))]
impl ToolSource for crate::WorkspaceTools {
    fn install(self, mut builder: ToolsBuilder) -> ToolsBuilder {
        if builder.tools.workspace_tools.is_some() {
            builder.duplicate_workspace = true;
        }
        builder.tools.workspace = true;
        builder.tools.workspace_tools = Some(self);
        builder
    }
}

#[cfg(not(target_family = "wasm"))]
pub trait RemoteRunnerTools: Sized {
    #[doc(hidden)]
    fn namespace(&self) -> Option<Arc<str>>;

    #[doc(hidden)]
    fn advertised_workspace(&self) -> Option<PathBuf>;

    #[doc(hidden)]
    fn into_catalog(self) -> Arc<dyn RemoteRunnerCatalog>;
}

#[cfg(not(target_family = "wasm"))]
pub trait AttachTarget {
    type Connector;

    #[doc(hidden)]
    fn connect_tools(self, tools: Tools) -> Self::Connector;
}

#[derive(Clone)]
pub struct Tools {
    exposure: ToolExposure,
    builtin_names: Arc<[Box<str>]>,
    workspace: bool,
    web_search: bool,
    image_generation: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(super) working_directory: Option<Arc<str>>,
    #[cfg(not(target_family = "wasm"))]
    pub(super) default_shell: Option<Arc<str>>,
    #[cfg(not(target_family = "wasm"))]
    process_environment: Arc<Vec<(OsString, OsString)>>,
    #[cfg(not(target_family = "wasm"))]
    session: Option<crate::SessionEnvironment>,
    #[cfg(not(target_family = "wasm"))]
    remote_http_client: Option<reqwest::Client>,
    pub(crate) registered: Vec<RegisteredTool>,
    #[cfg(not(target_family = "wasm"))]
    pub(super) provider_direct: Vec<Arc<dyn Tool>>,
    #[cfg(not(target_family = "wasm"))]
    pub(super) providers: Vec<Arc<dyn DynamicToolProvider>>,
    #[cfg(not(target_family = "wasm"))]
    attachable_providers: Vec<Arc<dyn Any + Send + Sync>>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) workspace_tools: Option<crate::WorkspaceTools>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) remote_runners: Vec<RemoteRoute>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) process_route_store: Option<Arc<dyn ProcessRouteStore>>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) runner_pool: Option<RunnerRouting>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) code_mode_executors: Option<nanocode_code_mode::remote::CodeModeExecutors>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) hand_calls: Option<Arc<dyn crate::embedded::HandCalls>>,
    #[cfg(not(target_family = "wasm"))]
    pub(super) deferred_tools_guidance_enabled: bool,

    pub(crate) embedded_host: Option<Arc<dyn crate::embedded::CodeModeHost>>,

    pub(crate) embedded_session_id: Option<Arc<str>>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) excluded_code_tools: HashSet<String>,

    code_mode_policies: Vec<Arc<dyn crate::CodeModeToolPolicy>>,

    recovery: Option<Arc<dyn crate::ToolRecovery>>,

    pub(crate) brief_code_tools: Arc<[String]>,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            exposure: ToolExposure::default(),
            builtin_names: Arc::from([]),
            workspace: true,
            web_search: true,
            image_generation: true,
            #[cfg(not(target_family = "wasm"))]
            working_directory: None,
            #[cfg(not(target_family = "wasm"))]
            default_shell: None,
            #[cfg(not(target_family = "wasm"))]
            process_environment: Arc::new(Vec::new()),
            #[cfg(not(target_family = "wasm"))]
            session: None,
            #[cfg(not(target_family = "wasm"))]
            remote_http_client: None,
            registered: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            provider_direct: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            providers: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            attachable_providers: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            workspace_tools: None,
            #[cfg(not(target_family = "wasm"))]
            remote_runners: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            process_route_store: None,
            #[cfg(not(target_family = "wasm"))]
            runner_pool: None,
            #[cfg(not(target_family = "wasm"))]
            code_mode_executors: None,
            #[cfg(not(target_family = "wasm"))]
            hand_calls: None,
            #[cfg(not(target_family = "wasm"))]
            deferred_tools_guidance_enabled: false,

            embedded_host: None,

            embedded_session_id: None,

            code_mode_policies: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            excluded_code_tools: HashSet::new(),
            recovery: None,
            brief_code_tools: Arc::from([]),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl fmt::Debug for Tools {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let remote_http_client_configured = self.remote_http_client.is_some();
        formatter
            .debug_struct("Tools")
            .field("exposure", &self.exposure)
            .field("workspace", &self.workspace)
            .field("web_search", &self.web_search)
            .field("image_generation", &self.image_generation)
            .field("working_directory", &self.working_directory)
            .field("default_shell", &self.default_shell)
            .field("process_environment_count", &self.process_environment.len())
            .field(
                "remote_http_client_configured",
                &remote_http_client_configured,
            )
            .field(
                "registered",
                &self
                    .registered
                    .iter()
                    .map(|tool| tool.handler.definition().name().to_owned())
                    .collect::<Vec<_>>(),
            )
            .field(
                "provider_direct",
                &self
                    .provider_direct
                    .iter()
                    .map(|tool| tool.definition().name().to_owned())
                    .collect::<Vec<_>>(),
            )
            .field("provider_count", &self.providers.len())
            .field("attachment_mcp_count", &self.attachment_mcp_count())
            .field(
                "workspace_tools_configured",
                &self.workspace_tools.is_some(),
            )
            .finish()
    }
}

#[cfg(target_family = "wasm")]
impl fmt::Debug for Tools {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("Tools");
        debug
            .field("exposure", &self.exposure)
            .field("workspace", &self.workspace)
            .field("web_search", &self.web_search)
            .field("image_generation", &self.image_generation)
            .field(
                "registered",
                &self
                    .registered
                    .iter()
                    .map(|tool| tool.handler.definition().name().to_owned())
                    .collect::<Vec<_>>(),
            );
        #[cfg(not(target_family = "wasm"))]
        debug.field(
            "workspace_tools_configured",
            &self.workspace_tools.is_some(),
        );
        debug.finish()
    }
}

impl Tools {
    pub fn with_builtin_names(
        self,
        names: impl IntoIterator<Item = impl Into<Box<str>>>,
    ) -> Result<Self, ToolsBuildError> {
        let mut builder = self.into_builder();
        builder.tools.builtin_names = names.into_iter().map(Into::into).collect();
        builder.build()
    }

    #[cfg(not(target_family = "wasm"))]
    pub const fn workspace_source(&self) -> Option<&crate::WorkspaceTools> {
        self.workspace_tools.as_ref()
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn with_workspace_source(mut self, source: crate::WorkspaceTools) -> Self {
        self.workspace_tools = Some(source);
        self
    }

    #[must_use]
    pub fn with_code_mode_policy(mut self, policy: Arc<dyn crate::CodeModeToolPolicy>) -> Self {
        if !self
            .code_mode_policies
            .iter()
            .any(|existing| Arc::ptr_eq(existing, &policy))
        {
            self.code_mode_policies.push(policy);
        }
        self
    }

    #[must_use]
    pub fn code_mode_policies(&self) -> Vec<Arc<dyn crate::CodeModeToolPolicy>> {
        #[cfg(not(target_family = "wasm"))]
        if let Some(policy) = self
            .workspace_tools
            .as_ref()
            .and_then(crate::WorkspaceTools::code_mode_policy)
            && !self
                .code_mode_policies
                .iter()
                .any(|existing| Arc::ptr_eq(existing, &policy))
        {
            let mut policies = self.code_mode_policies.clone();
            policies.push(policy);
            return policies;
        }
        self.code_mode_policies.clone()
    }

    #[must_use]
    pub const fn has_code_mode_host(&self) -> bool {
        self.embedded_host.is_some()
    }

    #[must_use]
    pub fn code_mode_host(mut self, host: impl crate::embedded::CodeModeHost) -> Self {
        self.embedded_host = Some(Arc::new(host));
        self
    }

    #[must_use]
    pub fn builder() -> ToolsBuilder {
        ToolsBuilder::default()
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn with_runner_pool(mut self, pool: impl Into<RunnerRouting>) -> Self {
        let pool = pool.into();
        if self.code_mode_executors.is_none() {
            self.code_mode_executors = pool.code_mode_executors().cloned();
        }
        self.runner_pool = Some(pool);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn with_code_mode_executors(
        mut self,
        executors: nanocode_code_mode::remote::CodeModeExecutors,
    ) -> Self {
        self.code_mode_executors = Some(executors);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub const fn code_mode_executors(
        &self,
    ) -> Option<&nanocode_code_mode::remote::CodeModeExecutors> {
        self.code_mode_executors.as_ref()
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn with_process_route_store(self, store: Arc<dyn ProcessRouteStore>) -> Self {
        self.into_builder().process_route_store(store).tools
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn with_default_process_route_store(mut self, store: Arc<dyn ProcessRouteStore>) -> Self {
        self.process_route_store.get_or_insert(store);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn attach<T: AttachTarget>(self, target: T) -> T::Connector {
        target.connect_tools(self)
    }

    #[doc(hidden)]
    #[cfg(not(target_family = "wasm"))]
    pub fn registered_tools(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.registered.iter().map(|registered| &registered.handler)
    }

    #[doc(hidden)]
    #[cfg(not(target_family = "wasm"))]
    pub fn attachable_providers(&self) -> &[Arc<dyn Any + Send + Sync>] {
        &self.attachable_providers
    }

    #[must_use]
    pub const fn into_builder(self) -> ToolsBuilder {
        ToolsBuilder {
            tools: self,
            duplicate_workspace: false,
        }
    }

    #[must_use]
    pub const fn exposure(&self) -> ToolExposure {
        self.exposure
    }

    pub(crate) fn recovery(&self) -> Option<Arc<dyn crate::ToolRecovery>> {
        self.recovery.clone()
    }

    #[must_use]
    pub const fn workspace_enabled(&self) -> bool {
        self.workspace
    }

    #[must_use]
    pub const fn web_search_enabled(&self) -> bool {
        self.web_search
    }

    #[must_use]
    pub const fn image_generation_enabled(&self) -> bool {
        self.image_generation
    }

    #[must_use]
    pub fn for_backend(self, session_id: &str, backend: &str) -> Self {
        if let Some(host) = &self.embedded_host {
            host.bind_session(session_id, backend);
        }
        #[cfg(not(target_family = "wasm"))]
        if let Some(executors) = &self.code_mode_executors {
            use crate::embedded::CodeModeHost;
            executors.bind_session(session_id, backend);
        }
        self
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn excluding_code_tools(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.excluded_code_tools.extend(names);
        self
    }

    #[must_use]
    pub fn brief_code_tools(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.brief_code_tools = names.into_iter().collect();
        self
    }

    #[must_use]
    pub fn for_session(mut self, session: &crate::SessionEnvironment) -> Self {
        self.embedded_session_id = Some(Arc::from(session.session_id()));
        if let Some(host) = &self.embedded_host {
            host.link_session(session.session_id(), session.root_session_id());
        }
        #[cfg(not(target_family = "wasm"))]
        {
            if let Some(executors) = &self.code_mode_executors {
                executors.link(session.session_id(), session.root_session_id());
            }
            for (name, value) in session.os_variables() {
                self.insert_process_environment(name, value);
            }
            self.session = Some(session.clone());
        }
        self
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub const fn session(&self) -> Option<&crate::SessionEnvironment> {
        self.session.as_ref()
    }

    #[doc(hidden)]
    #[cfg(not(target_family = "wasm"))]
    pub fn process_environment(&self) -> Arc<Vec<(OsString, OsString)>> {
        Arc::clone(&self.process_environment)
    }

    #[cfg(not(target_family = "wasm"))]
    fn insert_process_environment(&mut self, name: OsString, value: OsString) {
        let environment = Arc::make_mut(&mut self.process_environment);
        environment.retain(|(candidate, _)| candidate != &name);
        environment.push((name, value));
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn remote_http_client(&self) -> Option<reqwest::Client> {
        self.remote_http_client.clone()
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn start_providers(&self) {
        for provider in &self.providers {
            if let Some(session) = &self.session {
                provider.bind_session(session);
            }
            provider.start();
        }
    }

    #[cfg(target_family = "wasm")]
    pub const fn start_providers(&self) {}

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn has_dynamic_providers(&self) -> bool {
        !self.providers.is_empty() || self.runner_pool.is_some()
    }

    #[doc(hidden)]
    #[cfg(not(target_family = "wasm"))]
    pub fn has_unattachable_provider(&self) -> bool {
        self.providers.len() != self.attachment_mcp_count()
    }

    #[cfg(not(target_family = "wasm"))]
    const fn attachment_mcp_count(&self) -> usize {
        self.attachable_providers.len()
    }
}

#[derive(Default)]
pub struct ToolsBuilder {
    tools: Tools,
    duplicate_workspace: bool,
}

pub use nanocode_tools::ToolsBuildError;

impl ToolsBuilder {
    #[must_use]
    pub fn brief_code_tools(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.tools = self.tools.brief_code_tools(names);
        self
    }

    #[must_use]
    pub fn code_mode_host(mut self, host: impl crate::embedded::CodeModeHost) -> Self {
        self.tools = self.tools.code_mode_host(host);
        self
    }

    #[must_use]
    #[allow(
        clippy::should_implement_trait,
        reason = "builder composition is intentionally named add and is not arithmetic"
    )]
    pub fn add<S: ToolSource>(self, source: S) -> Self {
        source.install(self)
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn runner_pool(mut self, pool: impl Into<RunnerRouting>) -> Self {
        self.tools = self.tools.with_runner_pool(pool);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn code_mode_executors(
        mut self,
        executors: nanocode_code_mode::remote::CodeModeExecutors,
    ) -> Self {
        self.tools.code_mode_executors = Some(executors);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn process_route_store(mut self, store: Arc<dyn ProcessRouteStore>) -> Self {
        self.tools.process_route_store = Some(store);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn hand_calls(mut self, calls: Arc<dyn crate::embedded::HandCalls>) -> Self {
        self.tools.hand_calls = Some(calls);
        self
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn remote_runner_discovered<R: RemoteRunnerTools>(
        self,
        tools: R,
        mode: RemoteMode,
    ) -> Result<Self, AttachmentError> {
        let workspace = tools.advertised_workspace().ok_or_else(|| {
            AttachmentError::Catalog(
                "runner did not advertise a workspace; supply an explicit workspace override"
                    .into(),
            )
        })?;
        self.remote_runner(tools, workspace, mode)
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn remote_runner<R: RemoteRunnerTools>(
        mut self,
        tools: R,
        workspace: impl Into<PathBuf>,
        mode: RemoteMode,
    ) -> Result<Self, AttachmentError> {
        let workspace = workspace.into();
        let namespace = tools.namespace().filter(|_| workspace.is_absolute());
        let Some(namespace) = namespace else {
            return Err(AttachmentError::Catalog(
                "remote routing requires an absolute workspace and nonempty catalog".into(),
            ));
        };
        if self.tools.remote_runners.iter().any(|route| {
            route.namespace == namespace
                || route.mode == RemoteMode::Replace
                || mode == RemoteMode::Replace
        }) {
            return Err(AttachmentError::Catalog(
                "duplicate runner or replacement mixed with other runners".into(),
            ));
        }
        self.tools.workspace = true;
        if mode == RemoteMode::Replace {
            self.tools.working_directory = Some(Arc::from(format!("/{namespace}")));
        }
        self.tools.remote_runners.push(RemoteRoute {
            tools: tools.into_catalog(),
            namespace,
            workspace,
            mode,
        });
        Ok(self)
    }

    #[must_use]
    pub const fn exposure(mut self, exposure: ToolExposure) -> Self {
        self.tools.exposure = exposure;
        self
    }

    #[must_use]
    pub const fn without_defaults(mut self) -> Self {
        self.tools.workspace = false;
        self.tools.web_search = false;
        self.tools.image_generation = false;
        self
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    #[allow(clippy::missing_const_for_fn)]
    pub fn workspace(mut self, enabled: bool) -> Self {
        self.tools.workspace = enabled;
        #[cfg(not(target_family = "wasm"))]
        if !enabled {
            self.tools.workspace_tools = None;
        }
        self
    }

    #[must_use]
    #[cfg(target_family = "wasm")]
    pub const fn workspace(mut self, enabled: bool) -> Self {
        self.tools.workspace = enabled;
        self
    }

    #[must_use]
    pub const fn web_search(mut self, enabled: bool) -> Self {
        self.tools.web_search = enabled;
        self
    }

    #[must_use]
    pub const fn image_generation(mut self, enabled: bool) -> Self {
        self.tools.image_generation = enabled;
        self
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn working_directory(mut self, directory: impl Into<Arc<str>>) -> Self {
        self.tools.working_directory = Some(directory.into());
        self
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn default_shell(mut self, shell: impl Into<Arc<str>>) -> Self {
        self.tools.default_shell = Some(shell.into());
        self
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn process_environment<I, K, V>(mut self, variables: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        for (name, value) in variables {
            self.tools
                .insert_process_environment(name.into(), value.into());
        }
        self
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn remote_http_client(mut self, client: reqwest::Client) -> Self {
        self.tools.remote_http_client = Some(client);
        self
    }

    #[must_use]
    pub fn tool<T: Tool + 'static>(mut self, tool: T) -> Self {
        self.tools.registered.push(RegisteredTool {
            handler: Arc::new(tool),

            exposure: None,
        });
        self
    }

    #[must_use]
    pub fn tool_recovery(mut self, recovery: Arc<dyn crate::ToolRecovery>) -> Self {
        self.tools.recovery = Some(recovery);
        self
    }

    #[must_use]
    pub fn tool_with_exposure<T: Tool + 'static>(
        mut self,
        tool: T,
        exposure: ToolExposure,
    ) -> Self {
        self.tools.registered.push(RegisteredTool {
            handler: Arc::new(tool),

            exposure: Some(exposure),
        });
        self
    }

    #[doc(hidden)]
    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn attachable_provider<P: DynamicToolProvider + 'static>(
        mut self,
        provider: Arc<P>,
    ) -> Self {
        self.tools
            .providers
            .push(Arc::clone(&provider) as Arc<dyn DynamicToolProvider>);
        self.tools.attachable_providers.push(provider);
        self.refresh_provider_direct();
        self
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn provider<P: DynamicToolProvider + 'static>(self, provider: P) -> Self {
        self.shared_provider(Arc::new(provider))
    }

    #[must_use]
    #[cfg(not(target_family = "wasm"))]
    pub fn shared_provider(mut self, provider: Arc<dyn DynamicToolProvider>) -> Self {
        self.tools.providers.push(provider);
        self.refresh_provider_direct();
        self
    }

    #[allow(unused_mut, reason = "native provider refresh mutates this builder")]
    pub fn build(mut self) -> Result<Tools, ToolsBuildError> {
        if self.duplicate_workspace {
            return Err(ToolsBuildError::DuplicateSource("workspace"));
        }
        #[cfg(not(target_family = "wasm"))]
        self.refresh_provider_direct();
        #[cfg(not(target_family = "wasm"))]
        if self
            .tools
            .working_directory
            .as_deref()
            .is_some_and(|directory| directory.trim().is_empty())
        {
            return Err(ToolsBuildError::EmptyWorkingDirectory);
        }
        #[cfg(not(target_family = "wasm"))]
        if self
            .tools
            .default_shell
            .as_deref()
            .is_some_and(|shell| shell.trim().is_empty())
        {
            return Err(ToolsBuildError::EmptyDefaultShell);
        }
        let mut names = HashSet::with_capacity(self.tools.registered.len());

        let mut code_mode_names = HashMap::new();

        if self.tools.exposure.is_available_in_code_mode() {
            for name in self.tools.builtin_names.iter().map(AsRef::as_ref) {
                insert_code_mode_name(&mut code_mode_names, name)?;
            }
        }
        for tool in &self.tools.registered {
            let definition = tool.handler.definition();
            let name = definition.name();

            let exposure = tool.exposure.unwrap_or_else(|| self.tools.exposure());

            if exposure != ToolExposure::Hidden {
                validate_registered_tool_name(
                    name,
                    matches!(definition, ToolDefinition::ToolSearch { .. }),
                )?;
            } else if name.is_empty() {
                return Err(ToolsBuildError::EmptyName);
            }
            if host_owned_name(name)
                || (name == "tool_search"
                    && !matches!(definition, ToolDefinition::ToolSearch { .. }))
            {
                return Err(ToolsBuildError::ReservedName(name.into()));
            }
            if self
                .tools
                .builtin_names
                .iter()
                .any(|builtin| builtin.as_ref() == name)
            {
                return Err(ToolsBuildError::BuiltInName(name.into()));
            }
            if !names.insert(name.to_owned()) {
                return Err(ToolsBuildError::DuplicateName(name.into()));
            }

            if exposure.is_available_in_code_mode()
                && !matches!(definition, ToolDefinition::ToolSearch { .. })
            {
                insert_code_mode_name(&mut code_mode_names, name)?;
            }
        }
        #[cfg(not(target_family = "wasm"))]
        for tool in &self.tools.provider_direct {
            let definition = tool.definition();
            let name = definition.name();
            validate_registered_tool_name(
                name,
                matches!(definition, ToolDefinition::ToolSearch { .. }),
            )?;
            if host_owned_name(name) {
                return Err(ToolsBuildError::ReservedName(name.into()));
            }
            if self
                .tools
                .builtin_names
                .iter()
                .any(|builtin| builtin.as_ref() == name)
            {
                return Err(ToolsBuildError::BuiltInName(name.into()));
            }
            if !names.insert(name.to_owned()) {
                return Err(ToolsBuildError::DuplicateName(name.into()));
            }
            if self.tools.exposure.is_available_in_code_mode()
                && !matches!(definition, ToolDefinition::ToolSearch { .. })
            {
                insert_code_mode_name(&mut code_mode_names, name)?;
            }
        }
        #[cfg(not(target_family = "wasm"))]
        for provider in &self.tools.providers {
            for definition in provider.available_definitions() {
                let name = definition.name();
                validate_registered_tool_name(
                    name,
                    matches!(definition, ToolDefinition::ToolSearch { .. }),
                )?;
                if host_owned_name(name)
                    || (name == "tool_search"
                        && !matches!(definition, ToolDefinition::ToolSearch { .. }))
                {
                    return Err(ToolsBuildError::ReservedName(name.into()));
                }
                if self
                    .tools
                    .builtin_names
                    .iter()
                    .any(|builtin| builtin.as_ref() == name)
                {
                    return Err(ToolsBuildError::BuiltInName(name.into()));
                }
                if !names.insert(name.to_owned()) {
                    return Err(ToolsBuildError::DuplicateName(name.into()));
                }
                if !matches!(definition, ToolDefinition::ToolSearch { .. }) {
                    insert_code_mode_name(&mut code_mode_names, name)?;
                }
            }
        }
        Ok(self.tools)
    }

    #[cfg(not(target_family = "wasm"))]
    fn refresh_provider_direct(&mut self) {
        self.tools.deferred_tools_guidance_enabled = self.tools.providers.iter().any(|provider| {
            provider
                .direct_tools()
                .iter()
                .any(|tool| matches!(tool.definition(), ToolDefinition::ToolSearch { .. }))
        });
        self.tools.provider_direct = self
            .tools
            .providers
            .iter()
            .flat_map(|provider| provider.direct_tools_for_exposure(self.tools.exposure))
            .collect();
    }
}

#[cfg(not(target_family = "wasm"))]
#[derive(Debug, thiserror::Error)]
pub enum PublicToolCatalogError {
    #[error("invalid public tool name `{0}`")]
    InvalidName(Box<str>),
    #[error(
        "public tool names `{first}` and `{second}` both normalize to Code Mode name `{normalized}`"
    )]
    NormalizedNameCollision {
        first: Box<str>,
        second: Box<str>,
        normalized: Box<str>,
    },
}

#[cfg(not(target_family = "wasm"))]
pub fn validate_public_tool_catalog_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Result<(), PublicToolCatalogError> {
    let mut normalized_names = HashMap::new();
    for name in names {
        if !valid_public_tool_name(name) {
            return Err(PublicToolCatalogError::InvalidName(name.into()));
        }
        let normalized = normalize_public_tool_name(name);
        if let Some(first) = normalized_names.insert(normalized.clone(), name.to_owned()) {
            return Err(PublicToolCatalogError::NormalizedNameCollision {
                first: first.into(),
                second: name.into(),
                normalized: normalized.into(),
            });
        }
    }
    Ok(())
}

fn validate_registered_tool_name(
    name: &str,
    native_tool_search: bool,
) -> Result<(), ToolsBuildError> {
    if name.is_empty() {
        return Err(ToolsBuildError::EmptyName);
    }
    if !valid_public_tool_name_grammar(name) && !(native_tool_search && name == "tool_search") {
        return Err(ToolsBuildError::InvalidPublicName(name.into()));
    }
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn valid_public_tool_name(name: &str) -> bool {
    valid_public_tool_name_grammar(name) && !matches!(name, "exec" | "tool_search" | "wait")
}

fn valid_public_tool_name_grammar(name: &str) -> bool {
    name.len() <= 128
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn insert_code_mode_name(
    names: &mut HashMap<String, String>,
    name: &str,
) -> Result<(), ToolsBuildError> {
    let normalized = normalize_public_tool_name(name);
    if let Some(first) = names.insert(normalized.clone(), name.to_owned()) {
        return Err(ToolsBuildError::NormalizedNameCollision {
            first: first.into(),
            second: name.into(),
            normalized: normalized.into(),
        });
    }
    Ok(())
}

pub fn normalize_public_tool_name(name: &str) -> String {
    name.chars()
        .enumerate()
        .map(|(index, character)| {
            let valid = if index == 0 {
                character == '_' || character == '$' || character.is_ascii_alphabetic()
            } else {
                character == '_' || character == '$' || character.is_ascii_alphanumeric()
            };
            if valid { character } else { '_' }
        })
        .collect()
}

fn host_owned_name(name: &str) -> bool {
    matches!(name, "exec" | "wait")
}

impl nanocode_tools::ToolCollection for Tools {
    type Error = ToolsBuildError;

    fn add_shared_tool(self, tool: Arc<dyn Tool>) -> Result<Self, Self::Error> {
        let mut builder = self.into_builder();
        builder.tools.registered.push(RegisteredTool {
            handler: tool,

            exposure: None,
        });
        builder.build()
    }
}
