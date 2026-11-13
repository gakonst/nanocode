use std::{path::Path, sync::Arc};

use nanocode_agent_core::{AgentHandle, BackendId, ModelSelection, Result, backend::BackendFuture};
use nanocode_durability::{CheckpointFormat, SessionStore, SharedStore};
use nanocode_tool_host::{Tools, ToolsBuildError, image::ImageHistory};

pub type AgentResult = Result<(crate::Agent, crate::AgentEvents)>;
pub type ToolsFactory =
    Arc<dyn Fn(AgentHandle) -> std::result::Result<Tools, ToolsBuildError> + Send + Sync>;

#[derive(Clone)]
pub enum AgentTools {
    Shared(Tools),
    PerAgent(ToolsFactory),
}

impl AgentTools {
    pub const fn has_code_mode_host(&self) -> bool {
        match self {
            Self::Shared(tools) => tools.has_code_mode_host(),
            Self::PerAgent(_) => false,
        }
    }

    pub fn materialize(&self, handle: AgentHandle) -> std::result::Result<Tools, ToolsBuildError> {
        match self {
            Self::Shared(tools) => Ok(tools.clone()),
            Self::PerAgent(factory) => factory(handle),
        }
    }
}

#[derive(Clone, Default)]
pub struct Settings {
    pub instructions: Option<Arc<str>>,
    pub model_instructions: Vec<(ModelSelection, Arc<str>)>,
    pub images: Option<ImageHistory>,
    pub tools: Option<AgentTools>,
    #[cfg(not(target_family = "wasm"))]
    pub providers: Vec<Arc<dyn nanocode_tool_host::runtime::DynamicToolProvider>>,
}

impl Settings {
    pub fn or(self, fallback: &Self) -> Self {
        Self {
            instructions: self.instructions.or_else(|| fallback.instructions.clone()),
            model_instructions: match self.model_instructions.is_empty() {
                true => fallback.model_instructions.clone(),
                false => self.model_instructions,
            },
            images: self.images.or_else(|| fallback.images.clone()),
            tools: self.tools.or_else(|| fallback.tools.clone()),
            #[cfg(not(target_family = "wasm"))]
            providers: match self.providers.is_empty() {
                true => fallback.providers.clone(),
                false => self.providers,
            },
        }
    }
}

pub struct RuntimeRequest {
    pub request: crate::HarnessRequest,
    pub settings: Settings,
    pub defaults: Settings,
    pub workspace: Arc<Path>,
    pub subagents: Option<Arc<dyn DelegationTools>>,
    pub services: Arc<dyn HostServices>,
}

pub enum DelegationAuthority {
    Session,
    Turn,
}

pub trait DelegationTools: Send + Sync {
    fn tools(
        &self,
        parent: AgentHandle,
        authority: DelegationAuthority,
    ) -> Vec<Arc<dyn nanocode_tool_contracts::Tool>>;

    fn direct_tools(
        &self,
        parent: AgentHandle,
        authority: DelegationAuthority,
    ) -> Vec<Arc<dyn nanocode_tool_contracts::Tool>> {
        self.tools(parent, authority)
    }
}

pub trait RuntimeBackend: Send + Sync + 'static {
    fn id(&self) -> BackendId;
    fn is_harness(&self) -> bool {
        false
    }
    fn default_model(&self) -> ModelSelection;
    fn default_thinking(&self) -> Option<crate::Thinking> {
        None
    }
    fn validate_model(&self, model: &str) -> Result<()>;
    fn checkpoint_format(&self) -> Option<Arc<dyn CheckpointFormat>>;
    fn build(&self, request: RuntimeRequest) -> BackendFuture<AgentResult>;
}

pub trait HostServices: Send + Sync + 'static {
    fn activate(&self) {}
    fn shared_tools(
        &self,
        remote: bool,
    ) -> BackendFuture<Result<Vec<Arc<dyn nanocode_tool_contracts::Tool>>>>;
    fn prepare_tools(
        &self,
        handle: &AgentHandle,
        tools: Tools,
        store: Option<SharedStore>,
    ) -> std::result::Result<Tools, ToolsBuildError>;
    #[cfg(not(target_family = "wasm"))]
    fn process_executors(&self) -> Arc<dyn nanocode_process_contracts::ProcessExecutors>;
    #[cfg(not(target_family = "wasm"))]
    fn runner_routing(&self) -> Option<nanocode_tool_host::attachment::RunnerRouting>;
    #[cfg(not(target_family = "wasm"))]
    fn process_routes(
        &self,
        store: SharedStore,
    ) -> Arc<dyn nanocode_tool_host::attachment::ProcessRouteStore>;
    fn register<'a>(
        &'a self,
        agent: &'a crate::Agent,
        initial: bool,
    ) -> nanocode_tool_host::embedded::HostFuture<'a, Result<()>>;
    fn unregister(&self, session_id: &str);
    fn initial_notice(
        &self,
        _session_id: &str,
        _parent_session_id: Option<&str>,
    ) -> Option<String> {
        None
    }
    fn initial_notice_committed(&self, _agent: &crate::Agent, _text: String) {}
    fn unavailable(&self) -> Option<String>;
    fn close(&self);
    fn shutdown(&self) -> BackendFuture<Result<()>>;
}

pub trait RuntimeDefaults: Clone + Default + Send + Sync + 'static {
    type Codex: RuntimeBackend;
    type Claude: RuntimeBackend;
    type Host: HostServices;
    fn preferred_backend(existing: &[BackendId]) -> Option<BackendId>;
    fn default_backends(existing: &[BackendId]) -> Vec<Arc<dyn RuntimeBackend>>;
    fn default_store() -> nanocode_durability::Result<SessionStore>;
    fn default_store_async() -> Option<BackendFuture<nanocode_durability::Result<SessionStore>>> {
        None
    }
    fn build(self) -> Result<Arc<Self::Host>>;
    fn disable_machine_tools(&mut self);
}

#[cfg(not(target_family = "wasm"))]
pub trait NativeDefaults: RuntimeDefaults {
    type Computer;
    type Runner;
    type RunnerServerConfig;
    fn computer(&mut self, computer: Option<Self::Computer>);
    fn tool_environment(&mut self, environment: Vec<(std::ffi::OsString, std::ffi::OsString)>);
    fn runner_management(&mut self, enabled: bool);
    fn without_runners(&mut self) {}
    fn runner(&mut self, runner: Self::Runner);
    fn runner_server(&mut self, config: Self::RunnerServerConfig);
}

#[cfg(not(target_family = "wasm"))]
pub trait RunnerServices: HostServices {
    type Pool;
    type Runner;
    type ServerConfig;
    type Handle;
    type Server;
    type Event;
    type Info;
    type Error;
    fn runner_pool(&self) -> Self::Pool;
    fn initialize_runners(
        &self,
    ) -> nanocode_tool_host::embedded::HostFuture<'_, std::result::Result<(), Self::Error>>;
    fn runner_servers(&self) -> Vec<Self::Server>;
    fn connect_runner(
        &self,
        runner: Self::Runner,
    ) -> nanocode_tool_host::embedded::HostFuture<'_, std::result::Result<Self::Handle, Self::Error>>;
    fn listen_for_runners(
        &self,
        config: Self::ServerConfig,
    ) -> nanocode_tool_host::embedded::HostFuture<'_, std::result::Result<Self::Server, Self::Error>>;
    fn runner_events(&self) -> tokio::sync::mpsc::UnboundedReceiver<Self::Event>;
    fn list_runners(&self) -> Vec<Self::Info>;
    fn disconnect_runner<'a>(
        &'a self,
        name: &'a str,
    ) -> nanocode_tool_host::embedded::HostFuture<'a, std::result::Result<(), Self::Error>>;
}

pub trait DefaultRuntimeBackend: RuntimeBackend + Clone {
    fn backend_id() -> BackendId;
    fn native_default() -> Option<Self>;
    fn builtin_format() -> Arc<dyn CheckpointFormat>;
}
