use nanocode_agent::configuration::{
    DefaultRuntimeBackend, HostServices, RuntimeBackend, RuntimeDefaults,
};
use nanocode_agent_core::{Agent, AgentHandle, BackendId, Result, backend::BackendFuture};
use nanocode_durability::{SessionStore, SharedStore};
use nanocode_tool_host::{Tools, ToolsBuildError};
use std::{marker::PhantomData, sync::Arc};

#[cfg(not(target_family = "wasm"))]
use nanocode_agent::configuration::{NativeDefaults, RunnerServices};
#[cfg(not(target_family = "wasm"))]
use nanocode_agent_core::{NanocodeError, WeakAgent};
#[cfg(all(not(target_family = "wasm"), feature = "cua"))]
pub use nanocode_cua::{ComputerConfig, ComputerTools};
/// Built without the `cua` feature: no computer tools can exist.
#[cfg(all(not(target_family = "wasm"), not(feature = "cua")))]
#[derive(Clone)]
pub enum ComputerTools {}
#[cfg(not(target_family = "wasm"))]
pub use nanocode_tool_runtime::attachment::{
    AttachmentError, Runner, RunnerEvent, RunnerHandle, RunnerInfo, RunnerPool, RunnerProtocol,
    RunnerServer, RunnerServerConfig, RunnerSpec, RunnerStatus,
};
mod computer;
#[cfg(not(target_family = "wasm"))]
mod process_routes;
#[cfg(not(target_family = "wasm"))]
mod runners;

#[derive(Clone)]
pub struct Defaults<C, A> {
    backends: PhantomData<fn() -> (C, A)>,
    #[cfg(not(target_family = "wasm"))]
    computer: Option<ComputerTools>,
    #[cfg(not(target_family = "wasm"))]
    computer_disabled: bool,
    #[cfg(not(target_family = "wasm"))]
    management_disabled: bool,
    #[cfg(not(target_family = "wasm"))]
    runners_disabled: bool,
    #[cfg(target_os = "macos")]
    machine_tools_disabled: bool,
    #[cfg(not(target_family = "wasm"))]
    runners: Vec<Runner>,
    #[cfg(not(target_family = "wasm"))]
    servers: Vec<RunnerServerConfig>,
    #[cfg(not(target_family = "wasm"))]
    tool_environment: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl<C, A> Default for Defaults<C, A> {
    fn default() -> Self {
        Self {
            backends: PhantomData,
            #[cfg(not(target_family = "wasm"))]
            computer: None,
            #[cfg(not(target_family = "wasm"))]
            computer_disabled: false,
            #[cfg(not(target_family = "wasm"))]
            management_disabled: false,
            #[cfg(not(target_family = "wasm"))]
            runners_disabled: false,
            #[cfg(target_os = "macos")]
            machine_tools_disabled: false,
            #[cfg(not(target_family = "wasm"))]
            runners: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            servers: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            tool_environment: Vec::new(),
        }
    }
}

pub struct Services {
    computer: computer::Cell,
    #[cfg(not(target_family = "wasm"))]
    pool: RunnerPool,
    #[cfg(not(target_family = "wasm"))]
    agents: Arc<runners::RunnerAgents>,
    #[cfg(not(target_family = "wasm"))]
    runners_disabled: bool,
    #[cfg(not(target_family = "wasm"))]
    processes: nanocode_process::NativeProcessExecutors,
}

impl<C: DefaultRuntimeBackend, A: DefaultRuntimeBackend> RuntimeDefaults for Defaults<C, A> {
    type Codex = C;
    type Claude = A;
    type Host = Services;

    fn preferred_backend(existing: &[BackendId]) -> Option<BackendId> {
        [C::backend_id(), A::backend_id()]
            .into_iter()
            .find(|id| existing.contains(id))
    }

    fn default_backends(existing: &[BackendId]) -> Vec<Arc<dyn RuntimeBackend>> {
        let mut backends: Vec<Arc<dyn RuntimeBackend>> = Vec::new();
        if !existing.contains(&C::backend_id())
            && let Some(backend) = C::native_default()
        {
            backends.push(Arc::new(backend));
        }
        if !existing.contains(&A::backend_id())
            && let Some(backend) = A::native_default()
        {
            backends.push(Arc::new(backend));
        }
        backends
    }

    fn default_store() -> nanocode_durability::Result<SessionStore> {
        #[cfg(not(target_family = "wasm"))]
        let store = nanocode_home::AgentHome::from_env()
            .map_err(|error| nanocode_durability::StoreError::Backend(error.to_string()))
            .and_then(|home| nanocode_store_jsonl::JsonlStore::open_home(home.nanocode_home()))
            .map_err(nanocode_durability::Error::from)
            .and_then(SessionStore::new)?;
        #[cfg(target_family = "wasm")]
        let store = SessionStore::new(nanocode_durability::MemoryStore::new()?)?;
        store.register_format(C::builtin_format())?;
        store.register_format(A::builtin_format())?;
        Ok(store)
    }

    fn default_store_async() -> Option<BackendFuture<nanocode_durability::Result<SessionStore>>> {
        #[cfg(not(target_family = "wasm"))]
        {
            Some(Box::pin(async {
                let home = nanocode_home::AgentHome::from_env()
                    .map_err(|error| nanocode_durability::StoreError::Backend(error.to_string()))?;
                let store =
                    nanocode_store_jsonl::JsonlStore::open_home_async(home.nanocode_home()).await?;
                let store = SessionStore::new(store)?;
                store.register_format(C::builtin_format())?;
                store.register_format(A::builtin_format())?;
                Ok(store)
            }))
        }
        #[cfg(target_family = "wasm")]
        {
            None
        }
    }

    fn build(self) -> Result<Arc<Services>> {
        #[cfg(not(target_family = "wasm"))]
        {
            for runner in &self.runners {
                runner
                    .validate()
                    .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?;
            }
            for config in &self.servers {
                config
                    .validate()
                    .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?;
            }
            #[cfg(target_os = "macos")]
            if !self.machine_tools_disabled {
                nanocode_home::shell::warm_user_shell();
            }
            let pool = RunnerPool::new().with_model_management(!self.management_disabled);
            let agents = runners::RunnerAgents::new(pool.clone(), self.runners, self.servers);
            Ok(Arc::new(Services {
                computer: (!self.computer_disabled).then(|| computer::cell(self.computer)),
                pool,
                agents,
                runners_disabled: self.runners_disabled,
                processes: nanocode_process::NativeProcessExecutors::with_environment(
                    self.tool_environment,
                ),
            }))
        }
        #[cfg(target_family = "wasm")]
        {
            Ok(Arc::new(Services { computer: None }))
        }
    }

    fn disable_machine_tools(&mut self) {
        #[cfg(not(target_family = "wasm"))]
        {
            self.computer = None;
            self.computer_disabled = true;
            self.management_disabled = true;
            #[cfg(target_os = "macos")]
            {
                self.machine_tools_disabled = true;
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl<C: DefaultRuntimeBackend, A: DefaultRuntimeBackend> NativeDefaults for Defaults<C, A> {
    type Computer = ComputerTools;
    type Runner = Runner;
    type RunnerServerConfig = RunnerServerConfig;
    fn computer(&mut self, computer: Option<ComputerTools>) {
        self.computer_disabled = computer.is_none();
        self.computer = computer;
    }
    fn runner_management(&mut self, enabled: bool) {
        self.management_disabled = !enabled;
    }
    fn without_runners(&mut self) {
        self.runners_disabled = true;
        self.management_disabled = true;
        self.runners.clear();
        self.servers.clear();
    }
    fn runner(&mut self, runner: Runner) {
        self.runners.push(runner);
    }
    fn runner_server(&mut self, config: RunnerServerConfig) {
        self.servers.push(config);
    }
    fn tool_environment(&mut self, environment: Vec<(std::ffi::OsString, std::ffi::OsString)>) {
        self.tool_environment.extend(environment);
    }
}

impl HostServices for Services {
    #[cfg(not(target_family = "wasm"))]
    fn process_executors(&self) -> Arc<dyn nanocode_process_contracts::ProcessExecutors> {
        Arc::new(self.processes.clone())
    }

    fn activate(&self) {
        #[cfg(not(target_family = "wasm"))]
        self.agents.activate();
    }

    fn shared_tools(
        &self,
        remote: bool,
    ) -> BackendFuture<Result<Vec<Arc<dyn nanocode_tool_contracts::Tool>>>> {
        let computer = self.computer.clone();
        Box::pin(async move {
            if remote {
                return Ok(Vec::new());
            }
            Ok(computer::shared(computer::connect(computer).await.as_ref()))
        })
    }

    fn prepare_tools(
        &self,
        handle: &AgentHandle,
        tools: Tools,
        store: Option<SharedStore>,
    ) -> std::result::Result<Tools, ToolsBuildError> {
        #[cfg(not(target_family = "wasm"))]
        {
            self.agents
                .code_mode_host(handle.session_id(), tools.has_code_mode_host());
            if !tools.has_code_mode_host() {
                let mut tools = if self.runners_disabled {
                    tools
                } else {
                    tools.with_runner_pool(self.pool.clone())
                };
                if let Some(store) = store {
                    tools = tools.with_default_process_route_store(
                        process_routes::DurableProcessRoutes::new(store),
                    );
                }
                return Ok(tools);
            }
        }
        #[cfg(target_family = "wasm")]
        let _ = (handle, store);
        Ok(tools)
    }

    #[cfg(not(target_family = "wasm"))]
    fn runner_routing(&self) -> Option<nanocode_tool_host::attachment::RunnerRouting> {
        (!self.runners_disabled).then(|| self.pool.clone().into())
    }
    #[cfg(not(target_family = "wasm"))]
    fn process_routes(
        &self,
        store: SharedStore,
    ) -> Arc<dyn nanocode_tool_host::attachment::ProcessRouteStore> {
        process_routes::DurableProcessRoutes::new(store)
    }
    fn register<'a>(
        &'a self,
        agent: &'a Agent,
        initial: bool,
    ) -> nanocode_tool_host::embedded::HostFuture<'a, Result<()>> {
        #[cfg(not(target_family = "wasm"))]
        {
            Box::pin(self.agents.register(agent, initial))
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = (agent, initial);
            Box::pin(async { Ok(()) })
        }
    }

    fn unregister(&self, session_id: &str) {
        #[cfg(not(target_family = "wasm"))]
        self.agents.unregister(session_id);
        #[cfg(target_family = "wasm")]
        let _ = session_id;
    }

    #[cfg(not(target_family = "wasm"))]
    fn initial_notice(&self, session_id: &str, parent_session_id: Option<&str>) -> Option<String> {
        self.agents.initial_notice(session_id, parent_session_id)
    }

    #[cfg(not(target_family = "wasm"))]
    fn initial_notice_committed(&self, agent: &Agent, text: String) {
        self.agents.initial_notice_committed(agent, text);
    }

    fn unavailable(&self) -> Option<String> {
        computer::unavailable(&self.computer)
    }
    fn close(&self) {}
    fn shutdown(&self) -> BackendFuture<Result<()>> {
        #[cfg(not(target_family = "wasm"))]
        {
            let pool = self.pool.clone();
            Box::pin(async move {
                pool.shutdown()
                    .await
                    .map_err(|error| NanocodeError::Backend {
                        backend: "runners",
                        source: Arc::new(error),
                    })
            })
        }
        #[cfg(target_family = "wasm")]
        {
            Box::pin(async { Ok(()) })
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl RunnerServices for Services {
    type Pool = RunnerPool;
    type Runner = Runner;
    type ServerConfig = RunnerServerConfig;
    type Handle = RunnerHandle;
    type Server = RunnerServer;
    type Event = RunnerEvent;
    type Info = RunnerInfo;
    type Error = AttachmentError;
    fn runner_pool(&self) -> RunnerPool {
        self.agents.activate();
        self.pool.clone()
    }
    fn initialize_runners(
        &self,
    ) -> nanocode_tool_host::embedded::HostFuture<'_, std::result::Result<(), AttachmentError>>
    {
        Box::pin(self.agents.initialize())
    }
    fn runner_servers(&self) -> Vec<RunnerServer> {
        self.runner_pool().servers()
    }
    fn connect_runner(
        &self,
        runner: Runner,
    ) -> nanocode_tool_host::embedded::HostFuture<
        '_,
        std::result::Result<RunnerHandle, AttachmentError>,
    > {
        Box::pin(async move { self.runner_pool().connect(runner).await })
    }
    fn listen_for_runners(
        &self,
        config: RunnerServerConfig,
    ) -> nanocode_tool_host::embedded::HostFuture<
        '_,
        std::result::Result<RunnerServer, AttachmentError>,
    > {
        Box::pin(async move { self.runner_pool().listen(config).await })
    }
    fn runner_events(&self) -> tokio::sync::mpsc::UnboundedReceiver<RunnerEvent> {
        self.runner_pool().subscribe()
    }
    fn list_runners(&self) -> Vec<RunnerInfo> {
        self.runner_pool().list()
    }
    fn disconnect_runner<'a>(
        &'a self,
        name: &'a str,
    ) -> nanocode_tool_host::embedded::HostFuture<'a, std::result::Result<(), AttachmentError>>
    {
        Box::pin(async move { self.runner_pool().disconnect(name).await })
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn run_runner_with_workspace_tools(
    mut config: nanocode_tool_runtime::attachment::RunnerConfig,
    tools: Tools,
    factory: fn(std::path::PathBuf) -> nanocode_tool_host::WorkspaceTools,
) -> BackendFuture<std::result::Result<(), AttachmentError>> {
    Box::pin(async move {
        config.workspace = tokio::fs::canonicalize(&config.workspace)
            .await
            .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
        let tools = tools
            .into_builder()
            .without_defaults()
            .add(factory(config.workspace.clone()))
            .build()
            .map_err(|error| AttachmentError::Catalog(error.to_string().into()))?;
        nanocode_tool_runtime::attachment::run_runner_with_tools(config, tools).await
    })
}

impl<C: DefaultRuntimeBackend, A: DefaultRuntimeBackend> nanocode_agent::CheckpointFormats
    for Defaults<C, A>
{
    fn register_formats(store: &SessionStore) -> nanocode_durability::Result<()> {
        store.register_format(C::builtin_format())?;
        store.register_format(A::builtin_format())
    }
}
