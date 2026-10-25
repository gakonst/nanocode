use crate::{ClaudeBuilder, ClaudeClient};
use nanocode_agent_config::configuration::{
    AgentResult, AgentTools, RuntimeBackend, RuntimeRequest, Settings,
};
use nanocode_agent_core::{
    AgentHandle, BackendId, HarnessFamily, ModelSelection, NanocodeError, Result,
    backend::BackendFuture,
};
#[cfg(target_family = "wasm")]
use std::path::PathBuf;
use std::sync::Arc;
type Hook = Arc<dyn Fn(ClaudeBuilder) -> Result<ClaudeBuilder> + Send + Sync>;
#[derive(Clone)]
pub struct ClaudeHarness {
    client: ClaudeClient,
    settings: Settings,
    configure: Option<Hook>,
    headless: bool,
}
impl ClaudeHarness {
    #[must_use]
    pub fn new(client: ClaudeClient) -> Self {
        Self {
            client,
            settings: Settings::default(),
            configure: None,
            headless: false,
        }
    }

    #[must_use]
    pub const fn headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    #[must_use]
    pub fn instructions(mut self, instructions: impl Into<Arc<str>>) -> Self {
        self.settings.instructions = Some(instructions.into());
        self
    }

    #[must_use]
    pub fn tools(mut self, tools: nanocode_tool_host::Tools) -> Self {
        self.settings.tools = Some(AgentTools::Shared(tools));
        self
    }
    #[must_use]
    pub fn tools_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(
                AgentHandle,
            ) -> std::result::Result<
                nanocode_tool_host::Tools,
                nanocode_tool_host::ToolsBuildError,
            > + Send
            + Sync
            + 'static,
    {
        self.settings.tools = Some(AgentTools::PerAgent(Arc::new(factory)));
        self
    }

    #[must_use]
    pub fn configure<F>(mut self, configure: F) -> Self
    where
        F: Fn(ClaudeBuilder) -> Result<ClaudeBuilder> + Send + Sync + 'static,
    {
        self.configure = Some(Arc::new(configure));
        self
    }
}
impl From<ClaudeClient> for ClaudeHarness {
    fn from(client: ClaudeClient) -> Self {
        Self::new(client)
    }
}

#[cfg(not(target_family = "wasm"))]
impl Default for ClaudeHarness {
    fn default() -> Self {
        Self::new(ClaudeClient::default())
    }
}

impl RuntimeBackend for ClaudeHarness {
    fn is_harness(&self) -> bool {
        true
    }
    fn id(&self) -> BackendId {
        BackendId::new("claude")
    }
    fn default_model(&self) -> ModelSelection {
        HarnessFamily::Claude.default_model()
    }
    fn validate_model(&self, model: &str) -> Result<()> {
        ModelSelection::new(self.id(), model).validate()
    }
    fn checkpoint_format(&self) -> Option<Arc<dyn nanocode_durability::CheckpointFormat>> {
        Some(Arc::new(crate::durability::ClaudeCheckpointFormat))
    }
    fn build(&self, request: RuntimeRequest) -> BackendFuture<AgentResult> {
        let RuntimeRequest {
            request: spec,
            settings,
            defaults,
            workspace,
            subagents,
            services,
        } = request;
        let client = self.client.clone();
        let settings = settings.or(&self.settings.clone().or(&defaults));
        let configure = self.configure.clone();
        #[cfg(not(target_family = "wasm"))]
        let headless = self.headless;
        Box::pin(async move {
            let remote = settings
                .tools
                .as_ref()
                .is_some_and(AgentTools::has_code_mode_host);
            let fold_notice = !remote
                && configure.is_none()
                && spec.checkpoint.is_none()
                && !matches!(settings.tools, Some(AgentTools::PerAgent(_)));
            let host = services.clone();
            let shared = services.shared_tools(remote).await?;
            if spec.model.family() != HarnessFamily::Claude {
                return Err(NanocodeError::InvalidRequest(
                    "the Claude harness requires a Claude model".into(),
                ));
            }
            let mut builder = nanocode_agent_core::Agent::builder(crate::Claude::new(
                client,
                spec.model.as_str(),
            ))
            .thinking(spec.thinking)?
            .spawn_factory(spec.spawn_factory)
            .host_context(spec.host_context)
            .workspace(workspace.to_string_lossy().into_owned());
            if let Some(instructions) = settings.instructions {
                builder = builder.system(instructions.to_string());
            }
            for (model, instructions) in &settings.model_instructions {
                if model.family() == HarnessFamily::Claude {
                    builder = builder.model_instructions(model.as_str(), Arc::clone(instructions));
                }
            }
            if let Some(images) = settings.images {
                builder = builder.images(images);
            }
            if let Some(configure) = configure {
                builder = configure(builder)?.workspace(workspace.to_string_lossy().into_owned());
            }
            let tools = settings.tools;
            #[cfg(not(target_family = "wasm"))]
            let providers = settings.providers;
            let root = workspace.to_path_buf();
            let process_store = spec
                .durable_state
                .as_ref()
                .map(|state| state.shared_store());
            builder = builder.tools_factory(move |handle| {
                let selected = tools
                    .as_ref()
                    .map(|tools| tools.materialize(handle.clone()))
                    .transpose()
                    .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?;
                let selected = selected
                    .map(|tools| services.prepare_tools(&handle, tools, process_store.clone()))
                    .transpose()
                    .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?;
                let mut additive = if selected
                    .as_ref()
                    .is_some_and(nanocode_tool_host::Tools::has_code_mode_host)
                {
                    Vec::new()
                } else {
                    shared.clone()
                };
                if let Some(subagents) = &subagents {
                    additive.extend(subagents.tools(
                        handle.clone(),
                        nanocode_agent_config::configuration::DelegationAuthority::Turn,
                    ));
                }
                #[cfg(not(target_family = "wasm"))]
                {
                    crate::defaults::install_default_tools_with_providers(
                        root.clone(),
                        &handle,
                        selected,
                        additive,
                        process_store
                            .clone()
                            .map(|store| services.process_routes(store)),
                        services.runner_routing(),
                        services.process_executors().foreground(),
                        &providers,
                        headless,
                    )
                }
                #[cfg(target_family = "wasm")]
                {
                    claude_tools(root.clone(), &handle, selected, additive)
                }
            });
            builder = builder.code_only(cfg!(not(target_family = "wasm")) || remote);
            if let Some(checkpoint) = spec.checkpoint {
                builder = builder.resume(checkpoint)?;
            }

            if let Some(state) = spec.durable_state {
                use nanocode_durability::DurableAgentExt as _;
                if fold_notice
                    && let Some(record) = state
                        .described_record()
                        .filter(|record| record.initial.is_none())
                        .cloned()
                    && let Some(text) = host.initial_notice(
                        state.state_id(),
                        spec.parent.as_ref().map(AgentHandle::session_id),
                    )
                {
                    let (deferred, existing, _seed) =
                        builder
                            .clone()
                            .deferred_durability(&state, record, text.clone())?;
                    match nanocode_agent_core::Agent::persist_created(deferred.build()).await {
                        Ok((agent, events)) => {
                            host.initial_notice_committed(&agent, text);
                            return Ok((agent, events));
                        }
                        Err(_) if existing.load(std::sync::atomic::Ordering::SeqCst) => {}
                        Err(error) => return Err(error),
                    }
                }
                builder = builder.durability(state).await?;
            }
            builder.build()
        })
    }
}
#[cfg(target_family = "wasm")]
fn claude_tools(
    workspace: PathBuf,
    handle: &AgentHandle,
    selected: Option<nanocode_tool_host::Tools>,
    additive: Vec<Arc<dyn nanocode_tool_contracts::Tool>>,
) -> Result<crate::ClaudeTools> {
    use nanocode_tool_contracts::ToolCollection as _;
    if let Some(selected) = selected {
        let selected = additive
            .into_iter()
            .try_fold(selected, nanocode_tool_host::Tools::add_shared_tool)?;
        return crate::install_shared_tools(workspace, handle, selected);
    }
    additive.into_iter().try_fold(
        crate::ClaudeTools::new(),
        crate::ClaudeTools::add_shared_tool,
    )
}

impl nanocode_agent_config::configuration::DefaultRuntimeBackend for ClaudeHarness {
    fn backend_id() -> BackendId {
        BackendId::new("claude")
    }
    fn native_default() -> Option<Self> {
        #[cfg(not(target_family = "wasm"))]
        {
            Some(Self::default())
        }
        #[cfg(target_family = "wasm")]
        {
            None
        }
    }
    fn builtin_format() -> Arc<dyn nanocode_durability::CheckpointFormat> {
        Arc::new(crate::durability::ClaudeCheckpointFormat)
    }
}
