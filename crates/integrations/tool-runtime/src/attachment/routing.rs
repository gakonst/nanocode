use super::{
    ProcessRouteStore, RemoteMode, RemoteTool, RemoteTools, RunnerPool, RunnerRouting,
    management::RunnerManagement,
    process_routes::{ProcessRoutes, SessionMapping},
};
use crate::{
    Tool, ToolContext, ToolDefinition, ToolExposure, ToolInput, ToolResult, ToolSource,
    ToolsBuilder, host::RoutingCatalog,
};
use nanocode_tool_host::{
    CodeModeToolObservation, CodeModeToolPolicy, WorkspaceToolRouting,
    attachment::{RemoteRoute, RemoteRunnerCatalog, WorkspaceRouter},
    contract::ToolOutputContent,
};
use serde_json::Value;
use std::{
    any::Any,
    collections::{BTreeSet, HashSet},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

type Policies = Arc<[Arc<dyn CodeModeToolPolicy>]>;

struct RemoteCatalog(RemoteTools);

impl RemoteRunnerCatalog for RemoteCatalog {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn router(&self) -> Arc<dyn WorkspaceRouter> {
        Arc::new(AttachmentRouter)
    }
}

impl crate::RemoteRunnerTools for RemoteTools {
    fn namespace(&self) -> Option<Arc<str>> {
        self.iter().next().map(|tool| Arc::from(tool.namespace()))
    }

    fn advertised_workspace(&self) -> Option<PathBuf> {
        self.workspace().map(Path::to_path_buf)
    }

    fn into_catalog(self) -> Arc<dyn RemoteRunnerCatalog> {
        Arc::new(RemoteCatalog(self))
    }
}

impl ToolSource for RemoteTools {
    fn install(self, builder: ToolsBuilder) -> ToolsBuilder {
        self.into_iter().fold(builder, ToolsBuilder::tool)
    }
}

impl From<RunnerPool> for RunnerRouting {
    fn from(pool: RunnerPool) -> Self {
        let executors = pool.code_mode_executors();
        Self::new(Arc::new(pool), Arc::new(AttachmentRouter)).with_code_mode_executors(executors)
    }
}

struct AttachmentRouter;

impl WorkspaceRouter for AttachmentRouter {
    fn catalog_revision(&self, pool: &RunnerRouting) -> Option<u64> {
        pool.pool()
            .downcast_ref::<RunnerPool>()
            .and_then(RunnerPool::catalog_revision)
    }

    fn route(
        &self,
        catalog: &mut RoutingCatalog<'_>,
        routes: &[RemoteRoute],
        store: Option<Arc<dyn ProcessRouteStore>>,
        pool: Option<&RunnerRouting>,
    ) {
        let pool = pool.map(|pool| {
            pool.pool()
                .downcast_ref::<RunnerPool>()
                .expect("runner routing is constructed only from RunnerPool")
                .clone()
        });
        let routes = routes
            .iter()
            .map(|route| Route {
                tools: route
                    .tools
                    .as_any()
                    .downcast_ref::<RemoteCatalog>()
                    .expect("remote routes are constructed only from RemoteTools")
                    .0
                    .clone(),
                workspace: route.workspace.clone(),
                mode: route.mode,
            })
            .collect::<Vec<_>>();
        if let Some(pool) = &pool {
            catalog.add(
                Arc::new(pool.code_mode_executors().tool()),
                ToolExposure::CodeModeOnly,
            );
            if pool.model_management_enabled() {
                catalog.add(
                    Arc::new(RunnerManagement { pool: pool.clone() }),
                    ToolExposure::CodeModeOnly,
                );
            }
        }
        let local_names = catalog.names();
        let providers = catalog.providers().to_vec();
        let policies: Policies = catalog.policies().into();
        for remote in routes.iter().flat_map(|route| route.tools.iter()) {
            let definition = remote.native_definition();
            if !catalog.contains(definition.name()) {
                catalog.add(
                    Arc::new(RemoteOnlyTool {
                        definition,
                        providers: providers.clone(),
                    }),
                    ToolExposure::CodeModeOnly,
                );
            }
        }
        let mut known = routes.clone();
        if let Some(pool) = &pool {
            known.extend(pool.routes());
        }
        let guidance = Arc::new(RoutingGuidance::new(&policies, &known, &local_names));
        let routes = Arc::new(routes);
        catalog.add_policy(Arc::new(AdvertisedProcessSessions {
            local: Arc::clone(&policies),
            routes: Arc::clone(&routes),
            pool: pool.clone(),
        }));
        let sessions = Arc::new(ProcessRoutes::new(store));
        catalog.wrap(|tool, definition| {
            let name = definition.name();
            let routing = classify(&policies, &known, name);
            let routed = routing.is_some_and(|routing| pool.is_some() || !pool_only(routing))
                || (pool.is_some() && name.starts_with("mcp__"))
                || routes.iter().any(|route| {
                    route
                        .tools
                        .iter()
                        .any(|remote| remote.remote_name() == name)
                });
            routed.then(|| {
                Arc::new(RoutedTool {
                    local: Arc::clone(tool),
                    local_desktop: local_names.contains(name)
                        || providers.iter().any(|provider| provider.contains(name)),
                    routes: Arc::clone(&routes),
                    pool: pool.clone(),
                    sessions: Arc::clone(&sessions),
                    routing,
                    guidance: Arc::clone(&guidance),
                }) as Arc<dyn Tool>
            })
        });
        if let Some(pool) = pool {
            let provider = Arc::new(RunnerProvider {
                pool,
                routes,
                sessions,
                providers: catalog.providers().to_vec(),
                policies,
                local_names,
            });
            catalog.insert_provider_first(provider);
        }
    }
}

#[derive(Clone)]
pub(crate) struct Route {
    pub tools: RemoteTools,
    pub workspace: PathBuf,
    pub mode: RemoteMode,
}

pub(crate) struct RoutedTool {
    pub local: Arc<dyn Tool>,
    pub routes: Arc<Vec<Route>>,
    pub pool: Option<super::RunnerPool>,
    pub sessions: Arc<ProcessRoutes>,
    pub local_desktop: bool,
    pub routing: Option<WorkspaceToolRouting>,
    pub guidance: Arc<RoutingGuidance>,
}

pub(crate) struct RoutingGuidance {
    process_input: Option<Box<str>>,
    patch: Option<Box<str>>,
}

impl RoutingGuidance {
    fn new(
        policies: &[Arc<dyn CodeModeToolPolicy>],
        routes: &[Route],
        local: &HashSet<Box<str>>,
    ) -> Self {
        let names = local
            .iter()
            .map(AsRef::as_ref)
            .chain(
                routes
                    .iter()
                    .flat_map(|route| route.tools.iter().map(super::RemoteTool::remote_name)),
            )
            .collect::<BTreeSet<_>>();
        let find = |wanted| {
            names
                .iter()
                .find(|name| classify(policies, routes, name) == Some(wanted))
                .map(|name| Box::from(*name))
        };
        Self {
            process_input: find(WorkspaceToolRouting::ProcessInput),
            patch: find(WorkspaceToolRouting::Patch),
        }
    }
}

fn classify(
    policies: &[Arc<dyn CodeModeToolPolicy>],
    routes: &[Route],
    name: &str,
) -> Option<WorkspaceToolRouting> {
    policies
        .iter()
        .find_map(|policy| policy.workspace_routing(name))
        .or_else(|| {
            routes
                .iter()
                .flat_map(|route| route.tools.iter())
                .find(|tool| tool.remote_name() == name)
                .and_then(super::RemoteTool::routing)
        })
}

struct AdvertisedProcessSessions {
    local: Policies,
    routes: Arc<Vec<Route>>,
    pool: Option<super::RunnerPool>,
}

impl CodeModeToolPolicy for AdvertisedProcessSessions {
    fn append_observation_content(
        &self,
        calls: &[CodeModeToolObservation<'_>],
        content: &mut Vec<ToolOutputContent>,
    ) {
        let mut routes = self.routes.as_ref().clone();
        if let Some(pool) = &self.pool {
            routes.extend(pool.routes());
        }
        let advertised = |name: &str| {
            if self
                .local
                .iter()
                .any(|policy| policy.workspace_routing(name).is_some())
            {
                return None;
            }
            routes
                .iter()
                .flat_map(|route| route.tools.iter())
                .find(|tool| tool.remote_name() == name)
                .and_then(super::RemoteTool::routing)
        };
        let mut running = std::collections::BTreeMap::new();
        for observed in calls {
            let call = observed.call;
            match advertised(&call.name) {
                Some(WorkspaceToolRouting::ProcessInput) => {
                    if let Some(session_id) = call.input.get("session_id").and_then(Value::as_i64) {
                        running.remove(&session_id);
                    }
                }
                Some(WorkspaceToolRouting::ProcessStart) => {}
                _ => continue,
            }
            if let Some(session_id) = observed
                .process_trace
                .and_then(|trace| trace.session_id)
                .or_else(|| {
                    call.structured_result
                        .get("session_id")
                        .and_then(Value::as_i64)
                })
            {
                running.insert(session_id, call.name.as_str());
            }
        }
        for (session_id, origin) in running {
            if content
                .iter()
                .filter_map(|item| match item {
                    ToolOutputContent::InputText { text } => Some(text),
                    _ => None,
                })
                .any(|text| {
                    serde_json::from_str::<Value>(text).is_ok_and(|value| {
                        value.as_i64() == Some(session_id)
                            || value.get("session_id").and_then(Value::as_i64) == Some(session_id)
                    })
                })
            {
                continue;
            }
            let input = if advertised(origin) == Some(WorkspaceToolRouting::ProcessInput) {
                Some(origin)
            } else {
                let inputs = routes
                    .iter()
                    .filter(|route| route.tools.iter().any(|tool| tool.remote_name() == origin))
                    .flat_map(|route| route.tools.iter())
                    .map(super::RemoteTool::remote_name)
                    .filter(|name| advertised(name) == Some(WorkspaceToolRouting::ProcessInput))
                    .collect::<BTreeSet<_>>();
                (inputs.len() == 1)
                    .then(|| inputs.into_iter().next())
                    .flatten()
            };
            content.push(ToolOutputContent::InputText {
                text: match input {
                    Some(input) => format!("Nested process is still running with session ID {session_id}. Resume it with tools.{input}({{ session_id: {session_id} }})."),
                    None => format!("Nested process is still running with session ID {session_id}."),
                },
            });
        }
    }
}

const fn pool_only(routing: WorkspaceToolRouting) -> bool {
    matches!(
        routing,
        WorkspaceToolRouting::Shell | WorkspaceToolRouting::File
    )
}

const fn keeps_own_paths(routing: Option<WorkspaceToolRouting>) -> bool {
    matches!(
        routing,
        Some(
            WorkspaceToolRouting::ProcessStart
                | WorkspaceToolRouting::ProcessInput
                | WorkspaceToolRouting::Path
                | WorkspaceToolRouting::Patch
        )
    )
}

pub(crate) struct RemoteOnlyTool {
    pub definition: ToolDefinition,
    pub providers: Vec<Arc<dyn crate::DynamicToolProvider>>,
}
impl Tool for RemoteOnlyTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    fn supports_parallel_tool_calls(&self) -> bool {
        self.providers
            .iter()
            .find(|provider| provider.contains(self.definition.name()))
            .is_some_and(|provider| provider.supports_parallel_tool_calls(self.definition.name()))
    }
    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let ToolInput::Function(input) = input else {
                    return Err("dynamic provider requires object arguments".into());
                };
                let value: Value = serde_json::from_str(input.get())?;
                for provider in &self.providers {
                    if let Some(output) = provider
                        .execute(self.definition.name(), value.clone(), context)
                        .await
                    {
                        return Ok(output);
                    }
                }
                Err(format!(
                    "local workspace does not provide `{}`; select an attached runner workdir",
                    self.definition.name()
                )
                .into())
            };
            output
        })
    }
}

fn cua(name: &str) -> bool {
    matches!(
        name,
        "mcp__cua_repl__js" | "mcp__cua_repl__reset" | "mcp__cua_repl__js_reset"
    )
}

fn normalize(path: &Path) -> Result<PathBuf, crate::contract::ToolError> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if matches!(result.components().next_back(), Some(Component::Normal(_))) {
                    result.pop();
                } else if path.is_absolute() {
                    return Err("workdir traverses above its root".into());
                } else {
                    result.push("..");
                }
            }
            Component::CurDir => {}
            component => result.push(component.as_os_str()),
        }
    }
    Ok(result)
}

impl Route {
    pub(crate) fn namespace(&self) -> &str {
        self.tools.namespace()
    }
    pub(crate) fn remote(&self, name: &str) -> Option<RemoteTool> {
        self.tools
            .iter()
            .find(|tool| tool.remote_name() == name)
            .cloned()
    }
    pub(crate) fn mount(&self) -> PathBuf {
        PathBuf::from(format!("/{}", self.namespace()))
    }
}

impl RoutedTool {
    fn snapshot(&self) -> Vec<Route> {
        let mut routes = self.routes.as_ref().clone();
        if let Some(pool) = &self.pool {
            routes.extend(pool.routes());
        }
        routes
    }

    async fn check_mount(
        &self,
        path: &Path,
        routes: &[Route],
        session_id: &str,
    ) -> Result<(), crate::contract::ToolError> {
        if routes.iter().any(|route| path.starts_with(route.mount())) {
            return Ok(());
        }
        if let Some(name) = self.sessions.known_mount(session_id, path).await? {
            return Err(format!(
                "runner mount `/{name}` is unavailable; attach that runner before using it"
            )
            .into());
        }
        Ok(())
    }
}

impl Tool for RoutedTool {
    fn definition(&self) -> ToolDefinition {
        let mut definition = self.local.definition();
        if self.pool.is_none() {
            let mut guidance = format!(
                " Workspace routing: default cwd executes {}. Remote mounts: {}. Paths under a mount map to its workspace.",
                if self
                    .routes
                    .iter()
                    .any(|route| route.mode == RemoteMode::Replace)
                {
                    "on the replacement runner"
                } else {
                    "locally"
                },
                self.routes
                    .iter()
                    .map(|route| format!(
                        "{} -> {}",
                        route.mount().display(),
                        route.workspace.display()
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if let Some(name) = &self.guidance.process_input {
                guidance.push_str(&format!(" {name} routes by the returned session_id."));
            }
            if let Some(name) = &self.guidance.patch {
                guidance.push_str(&format!(
                    " {name} routes by absolute file paths; do not mix machines in one patch."
                ));
            }
            match &mut definition {
                ToolDefinition::Function { description, .. }
                | ToolDefinition::Custom { description, .. } => {
                    *description = format!("{description}{guidance}").into();
                }
                _ => {}
            }
        }
        if let ToolDefinition::Function { parameters, .. } = &mut definition {
            let mut schema = parameters.as_value().clone();
            if let Some(object) = schema.as_object_mut()
                && !keeps_own_paths(self.routing)
                && (cua(definition_name(&self.local).as_str()) || !self.snapshot().is_empty())
                && let Some(properties) = object
                    .entry("properties")
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
            {
                properties.insert("workdir".into(), serde_json::json!({"type":"string","description":"Logical workspace root selecting the local host or a remote runner."}));
            }
            if cua(definition_name(&self.local).as_str())
                && let Some(object) = schema.as_object_mut()
            {
                let required = object
                    .entry("required")
                    .or_insert_with(|| serde_json::json!([]));
                if let Some(required) = required.as_array_mut()
                    && !required.iter().any(|field| field == "workdir")
                {
                    required.push("workdir".into());
                }
                if definition_name(&self.local) == "mcp__cua_repl__js" {
                    object.insert("required".into(), serde_json::json!(["workdir"]));
                }
            }
            *parameters = schema.into();
        }
        definition
    }
    fn supports_parallel_tool_calls(&self) -> bool {
        self.pool.is_none()
            && self.local.supports_parallel_tool_calls()
            && self
                .routes
                .iter()
                .filter_map(|route| route.remote(self.local.definition().name()))
                .all(|remote| remote.supports_parallel_tool_calls())
    }
    fn end_turn<'life0, 'life1, 'life2, 'life3, 'async_trait>(
        &'life0 self,
        session_id: &'life1 str,
        turn_id: &'life2 str,
        hook: &'life3 str,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<(), crate::contract::ToolError>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Result<(), crate::contract::ToolError> = {
                self.local.end_turn(session_id, turn_id, hook).await?;
                let routes = self.snapshot();
                for remote in routes
                    .iter()
                    .filter_map(|route| route.remote(self.local.definition().name()))
                {
                    remote.end_turn(session_id, turn_id, hook).await?;
                }
                Ok(())
            };
            output
        })
    }
    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = { self.execute_routes(input, context, self.snapshot()).await };
            output
        })
    }
}

impl RoutedTool {
    async fn execute_routes(
        &self,
        input: ToolInput,
        context: ToolContext<'_>,
        routes: Vec<Route>,
    ) -> ToolResult {
        if let Some(pool) = &self.pool {
            self.sessions
                .remember_mounts(context.session_id(), &pool.reserved_mounts())
                .await?;
            if let Some(name) = pool.replacement()
                && !routes.iter().any(|route| route.namespace() == name)
            {
                return Err(format!("replacement runner `{name}` is unavailable").into());
            }
        }
        let definition = self.local.definition();
        let name = definition.name();
        let mut selected = routes
            .iter()
            .position(|route| route.mode == RemoteMode::Replace);
        let mut requested = None;
        let input = match input {
            ToolInput::Function(raw) => {
                let mut value: Value = serde_json::from_str(raw.get())?;
                if cua(name) {
                    let workdir = value
                        .get("workdir")
                        .and_then(Value::as_str)
                        .ok_or("CUA requires an explicit workdir selecting an attached desktop")?;
                    let path = normalize(Path::new(workdir))?;
                    selected = routes
                        .iter()
                        .position(|route| path.starts_with(route.mount()));
                    if let Some(index) = selected {
                        let remote = routes[index].remote(name).ok_or_else(|| {
                            format!("selected root `{workdir}` has no desktop providing `{name}`")
                        })?;
                        if name == "mcp__cua_repl__js"
                            && value.as_object().is_some_and(|object| object.len() == 1)
                        {
                            return Ok(crate::ToolOutput::json(&serde_json::json!({"workdir":workdir, "tools":routes[index].tools.iter().filter(|tool| cua(tool.remote_name())).map(|tool| tool.native_definition()).collect::<Vec<_>>() })).with_structured_result(serde_json::json!({"workdir":workdir,"definition":remote.native_definition()})));
                        }
                    } else {
                        self.check_mount(&path, &routes, context.session_id())
                            .await?;
                        if !self.local_desktop
                            || routes.iter().any(|route| route.mode == RemoteMode::Replace)
                        {
                            return Err(format!(
                                "selected root `{workdir}` has no attached desktop"
                            )
                            .into());
                        }
                        if name == "mcp__cua_repl__js"
                            && value.as_object().is_some_and(|object| object.len() == 1)
                        {
                            return Ok(crate::ToolOutput::json(
                                &serde_json::json!({"workdir":workdir, "definition":self.local.definition()}),
                            ));
                        }
                    }
                }
                if self.routing == Some(WorkspaceToolRouting::ProcessInput) {
                    if let Some(session) = value.get("session_id").and_then(Value::as_i64) {
                        let stored = self
                            .sessions
                            .get(context.session_id(), i32::try_from(session)?)
                            .await?;
                        selected = if let Some(namespace) = &stored.namespace {
                            let index = routes
                                .iter()
                                .position(|route| route.namespace() == &**namespace)
                                .ok_or("process runner is no longer attached")?;
                            let current = routes[index]
                                .remote(name)
                                .and_then(|tool| tool.runtime_id());
                            if current != stored.runtime_id {
                                return Err("process session belongs to a restarted runner".into());
                            }
                            Some(index)
                        } else {
                            if stored.runtime_id.as_deref() != Some(self.sessions.runtime_id()) {
                                return Err(
                                    "local process session belongs to a previous SDK runtime"
                                        .into(),
                                );
                            }
                            None
                        };
                        requested = Some((session, stored.original));
                        value["session_id"] = stored.original.into();
                    }
                } else {
                    let key = if value.get("workdir").is_some() {
                        "workdir"
                    } else if value.get("cwd").is_some() {
                        "cwd"
                    } else if value.get("path").is_some() {
                        "path"
                    } else if value.get("file_path").is_some() {
                        "file_path"
                    } else {
                        "workdir"
                    };
                    if let Some(path) = value.get(key).and_then(Value::as_str) {
                        let path = normalize(Path::new(path))?;
                        self.check_mount(&path, &routes, context.session_id())
                            .await?;
                        for (index, route) in routes.iter().enumerate() {
                            if let Ok(relative) = path.strip_prefix(route.mount()) {
                                selected = Some(index);
                                value[key] = route
                                    .workspace
                                    .join(relative)
                                    .to_string_lossy()
                                    .into_owned()
                                    .into();
                                break;
                            }
                        }
                    }
                }
                if let Some(index) = selected {
                    let route = &routes[index];
                    for key in ["path", "file_path"] {
                        if let Some(path) = value.get(key).and_then(Value::as_str) {
                            let path = normalize(Path::new(path))?;
                            if let Ok(relative) = path.strip_prefix(route.mount()) {
                                value[key] = route
                                    .workspace
                                    .join(relative)
                                    .to_string_lossy()
                                    .into_owned()
                                    .into();
                            }
                        }
                    }
                }
                if self.routing == Some(WorkspaceToolRouting::Shell)
                    && let Some(workdir) = value.get("workdir").and_then(Value::as_str)
                    && let Some(command) = value.get("command").and_then(Value::as_str)
                {
                    value["command"] =
                        format!("cd '{}' && {command}", workdir.replace('\'', "'\\''")).into();
                }
                if !keeps_own_paths(self.routing)
                    && let Some(object) = value.as_object_mut()
                {
                    object.remove("workdir");
                }
                ToolInput::Function(serde_json::value::to_raw_value(&value)?)
            }
            ToolInput::Freeform(mut patch) => {
                let mut machine = None;
                for line in patch.lines() {
                    for prefix in [
                        "*** Add File: ",
                        "*** Update File: ",
                        "*** Delete File: ",
                        "*** Move to: ",
                    ] {
                        if let Some(path) = line.strip_prefix(prefix) {
                            let path = normalize(Path::new(path))?;
                            self.check_mount(&path, &routes, context.session_id())
                                .await?;
                            let route = routes
                                .iter()
                                .position(|route| path.starts_with(route.mount()))
                                .or(selected);
                            if machine.is_some_and(|previous| previous != route) {
                                return Err(
                                    "a patch cannot span local and remote workspaces".into()
                                );
                            }
                            machine = Some(route);
                        }
                    }
                }
                selected = machine.unwrap_or(selected);
                if let Some(index) = selected {
                    let route = &routes[index];
                    patch = patch
                        .lines()
                        .map(|line| {
                            for prefix in [
                                "*** Add File: ",
                                "*** Update File: ",
                                "*** Delete File: ",
                                "*** Move to: ",
                            ] {
                                if let Some(path) = line.strip_prefix(prefix)
                                    && let Ok(relative) =
                                        normalize(Path::new(path)).and_then(|path| {
                                            Ok(path.strip_prefix(route.mount())?.to_path_buf())
                                        })
                                {
                                    return format!(
                                        "{prefix}{}",
                                        route.workspace.join(relative).display()
                                    );
                                }
                            }
                            line.to_owned()
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                ToolInput::Freeform(patch)
            }
        };
        let (mut output, runtime_id) = if let Some(index) = selected {
            routes[index]
                .remote(name)
                .ok_or_else(|| format!("remote runner does not provide {name}"))?
                .execute_with_runtime(input, context)
                .await?
        } else {
            (self.local.execute(input, context).await?, None)
        };
        if let Some((session, original)) = requested
            && let crate::contract::ToolOutputBody::Text(text) = &output.output
            && *text == format!("write_stdin failed: Unknown process id {original}")
        {
            output.output = crate::contract::ToolOutputBody::Text(format!(
                "write_stdin failed: Unknown process id {session}"
            ));
        }
        if matches!(
            self.routing,
            Some(WorkspaceToolRouting::ProcessStart | WorkspaceToolRouting::ProcessInput)
        ) {
            {
                let mut result = output.structured_result();
                if let Some(original) = result.get("session_id").and_then(Value::as_i64) {
                    let mapping = SessionMapping {
                        original,
                        namespace: selected.map(|index| routes[index].namespace().into()),
                        runtime_id: if selected.is_some() {
                            runtime_id.clone()
                        } else {
                            Some(self.sessions.runtime_id().into())
                        },
                    };
                    let id = self.sessions.insert(context.session_id(), mapping).await?;
                    result["session_id"] = id.into();
                    if let Some(trace) = output.process_trace().copied() {
                        output = output.with_process_trace(
                            trace.exit_code,
                            Some(i64::from(id)),
                            trace.original_token_count,
                            trace.output_bytes,
                            trace.wall_time_seconds,
                        );
                    }
                    output = output.with_structured_result(result.clone());
                    output.output = match &output.output {
                        crate::contract::ToolOutputBody::Text(text)
                            if text.contains(&format!(
                                "Process running with session ID {original}
"
                            )) =>
                        {
                            crate::contract::ToolOutputBody::Text(text.replacen(
                                &format!(
                                    "Process running with session ID {original}
"
                                ),
                                &format!(
                                    "Process running with session ID {id}
"
                                ),
                                1,
                            ))
                        }
                        _ => crate::contract::ToolOutputBody::Text(result.to_string()),
                    };
                }
            }
        }
        Ok(output)
    }
}

fn definition_name(tool: &Arc<dyn Tool>) -> String {
    tool.definition().name().to_owned()
}

pub(crate) struct RunnerProvider {
    pub pool: super::RunnerPool,
    pub routes: Arc<Vec<Route>>,
    pub sessions: Arc<ProcessRoutes>,
    pub providers: Vec<Arc<dyn crate::DynamicToolProvider>>,
    pub policies: Policies,
    pub local_names: HashSet<Box<str>>,
}

struct Snapshot {
    pool: Vec<Route>,
    effective: Vec<Route>,
    guidance: Arc<RoutingGuidance>,
}

impl RunnerProvider {
    fn snapshot(&self) -> Snapshot {
        let pool = self.pool.routes();
        let mut effective = self.routes.as_ref().clone();
        effective.extend(pool.iter().cloned());
        let guidance = Arc::new(RoutingGuidance::new(
            &self.policies,
            &effective,
            &self.local_names,
        ));
        Snapshot {
            pool,
            effective,
            guidance,
        }
    }

    fn definitions(&self, snapshot: &Snapshot) -> Vec<ToolDefinition> {
        let mut seen = HashSet::new();
        self.providers
            .iter()
            .flat_map(|provider| provider.available_definitions())
            .chain(
                snapshot
                    .pool
                    .iter()
                    .flat_map(|route| route.tools.iter().map(|tool| tool.native_definition())),
            )
            .filter(|definition| seen.insert(definition.name().to_owned()))
            .collect()
    }

    fn definition(&self, snapshot: &Snapshot, name: &str) -> Option<ToolDefinition> {
        self.providers
            .iter()
            .filter(|provider| provider.contains(name))
            .flat_map(|provider| provider.available_definitions())
            .find(|definition| definition.name() == name)
            .or_else(|| {
                snapshot
                    .pool
                    .iter()
                    .flat_map(|route| route.tools.iter())
                    .find(|tool| tool.remote_name() == name)
                    .map(RemoteTool::native_definition)
            })
    }

    fn local_provider(
        &self,
        snapshot: &Snapshot,
        name: &str,
    ) -> Option<&Arc<dyn crate::DynamicToolProvider>> {
        if cua(name)
            || self.pool.replacement().is_some()
            || classify(&self.policies, &snapshot.effective, name).is_some()
            || snapshot
                .effective
                .iter()
                .any(|route| route.mode == RemoteMode::Replace)
        {
            return None;
        }
        self.providers
            .iter()
            .find(|provider| provider.contains(name))
    }

    fn routed(&self, definition: ToolDefinition, snapshot: &Snapshot) -> RoutedTool {
        RoutedTool {
            local_desktop: self
                .providers
                .iter()
                .any(|provider| provider.contains(definition.name())),
            routing: classify(&self.policies, &snapshot.effective, definition.name()),
            local: Arc::new(RemoteOnlyTool {
                definition,
                providers: self.providers.clone(),
            }),
            routes: self.routes.clone(),
            pool: Some(self.pool.clone()),
            sessions: self.sessions.clone(),
            guidance: Arc::clone(&snapshot.guidance),
        }
    }
}

impl crate::DynamicToolProvider for RunnerProvider {
    fn start(&self) {}
    fn direct_tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }
    fn model_catalog_visible(&self) -> bool {
        false
    }
    fn contains(&self, name: &str) -> bool {
        self.providers
            .iter()
            .any(|provider| provider.contains(name))
            || self
                .pool
                .routes()
                .iter()
                .any(|route| route.tools.iter().any(|tool| tool.remote_name() == name))
    }
    fn available_definitions(&self) -> Vec<ToolDefinition> {
        let snapshot = self.snapshot();
        self.definitions(&snapshot)
            .into_iter()
            .map(|definition| self.routed(definition, &snapshot).definition())
            .collect()
    }
    fn execute<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        name: &'life1 str,
        input: Value,
        context: ToolContext<'life2>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<crate::ToolOutput>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Option<crate::ToolOutput> = {
                let snapshot = self.snapshot();
                let definition = self.definition(&snapshot, name)?;
                let routed = self.routed(definition, &snapshot);
                Some(match serde_json::value::to_raw_value(&input) {
                    Ok(raw) => match routed
                        .execute_routes(ToolInput::Function(raw), context, snapshot.effective)
                        .await
                    {
                        Ok(output) => output,
                        Err(error) => crate::ToolOutput::error(error.to_string()),
                    },
                    Err(error) => crate::ToolOutput::error(error.to_string()),
                })
            };
            output
        })
    }
    fn resumes(&self, name: &str) -> bool {
        self.local_provider(&self.snapshot(), name)
            .is_some_and(|provider| provider.resumes(name))
    }
    fn recover<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        name: &'life1 str,
        mut input: Value,
        context: ToolContext<'life2>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<crate::ToolOutput>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Option<crate::ToolOutput> = {
                let snapshot = self.snapshot();
                let provider = self.local_provider(&snapshot, name)?;
                let key = ["workdir", "cwd", "path", "file_path"]
                    .into_iter()
                    .find(|key| input.get(key).is_some());
                if let Some(path) = key.and_then(|key| input.get(key)).and_then(Value::as_str) {
                    let path = normalize(Path::new(path)).ok()?;
                    if snapshot
                        .effective
                        .iter()
                        .any(|route| path.starts_with(route.mount()))
                    {
                        return None;
                    }
                }
                if let Some(object) = input.as_object_mut() {
                    object.remove("workdir");
                }
                provider.recover(name, input, context).await
            };
            output
        })
    }
}
