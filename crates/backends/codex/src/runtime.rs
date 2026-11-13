use crate::{CodexBuilder, OpenAi};
use nanocode_agent_config::{
    HarnessRequest,
    configuration::{AgentResult, AgentTools, RuntimeBackend, RuntimeRequest, Settings},
};
use nanocode_agent_core::{
    AgentHandle, BackendId, HarnessFamily, ModelSelection, ModelTransport, NanocodeError, Result,
    backend::BackendFuture,
};
use std::sync::Arc;
type Hook = Arc<dyn Fn(CodexBuilder) -> Result<CodexBuilder> + Send + Sync>;
type Route = Arc<dyn Fn(&HarnessRequest) -> Option<OpenAi> + Send + Sync>;
type Direct = Arc<dyn Fn(&HarnessRequest) -> bool + Send + Sync>;
#[derive(Clone)]
pub struct CodexHarness {
    client: OpenAi,
    settings: Settings,
    configure: Option<Hook>,
    client_for: Option<Route>,
    direct_tools_for: Option<Direct>,
    direct_subagents: bool,
}

impl CodexHarness {
    #[must_use]
    pub fn new(client: OpenAi) -> Self {
        Self {
            client,
            settings: Settings::default(),
            configure: None,
            client_for: None,
            direct_tools_for: None,
            direct_subagents: false,
        }
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
        F: Fn(CodexBuilder) -> Result<CodexBuilder> + Send + Sync + 'static,
    {
        self.configure = Some(Arc::new(configure));
        self
    }

    #[must_use]
    pub fn client_for<F>(mut self, client_for: F) -> Self
    where
        F: Fn(&HarnessRequest) -> Option<OpenAi> + Send + Sync + 'static,
    {
        self.client_for = Some(Arc::new(client_for));
        self
    }

    #[must_use]
    pub fn direct_tools_for<F>(mut self, direct_tools_for: F) -> Self
    where
        F: Fn(&HarnessRequest) -> bool + Send + Sync + 'static,
    {
        self.direct_tools_for = Some(Arc::new(direct_tools_for));
        self
    }

    #[must_use]
    pub const fn direct_subagents(mut self, enabled: bool) -> Self {
        self.direct_subagents = enabled;
        self
    }
}

impl From<OpenAi> for CodexHarness {
    fn from(client: OpenAi) -> Self {
        Self::new(client)
    }
}

#[cfg(not(target_family = "wasm"))]
impl Default for CodexHarness {
    fn default() -> Self {
        Self::new(OpenAi::default())
    }
}

impl RuntimeBackend for CodexHarness {
    fn is_harness(&self) -> bool {
        true
    }
    fn id(&self) -> BackendId {
        BackendId::new("codex")
    }
    fn default_model(&self) -> ModelSelection {
        HarnessFamily::Codex.default_model()
    }
    fn validate_model(&self, model: &str) -> Result<()> {
        ModelSelection::new(self.id(), model).validate()
    }
    fn checkpoint_format(&self) -> Option<Arc<dyn nanocode_durability::CheckpointFormat>> {
        Some(Arc::new(crate::durability::CodexCheckpointFormat))
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
        let client = self
            .client_for
            .as_ref()
            .and_then(|client_for| client_for(&spec))
            .unwrap_or_else(|| self.client.clone());
        let settings = settings.or(&self.settings.clone().or(&defaults));
        let configure = self.configure.clone();
        let direct = self
            .direct_tools_for
            .as_ref()
            .is_some_and(|direct_tools_for| direct_tools_for(&spec));
        let direct_subagents = self.direct_subagents;
        Box::pin(async move {
            let remote = settings
                .tools
                .as_ref()
                .is_some_and(AgentTools::has_code_mode_host);
            let subagents = subagents
                .filter(|_| !direct || remote || direct_subagents || spec.parent.is_some());
            let fold_notice = !remote
                && configure.is_none()
                && spec.checkpoint.is_none()
                && !matches!(settings.tools, Some(AgentTools::PerAgent(_)));
            let host = services.clone();
            let shared = services.shared_tools(remote).await?;
            let Some(model) = spec.model.as_codex() else {
                return Err(NanocodeError::InvalidRequest(
                    "the Codex harness requires a Codex model".into(),
                ));
            };
            let mut builder = crate::Agent::builder(client)
                .model(model)
                .thinking(spec.thinking)
                .spawn_factory(spec.spawn_factory)
                .host_context(spec.host_context)
                .workspace(workspace.to_path_buf());
            if let Some(instructions) = settings.instructions {
                builder = builder.instructions(instructions);
            }
            for (model, instructions) in &settings.model_instructions {
                if let Some(model) = model.as_codex() {
                    builder = builder.model_instructions(model, Arc::clone(instructions));
                }
            }
            if let Some(images) = settings.images {
                builder = builder.images(images);
            }
            if let Some(configure) = configure {
                builder = configure(builder)?.workspace(workspace.to_path_buf());
            }
            if let Some(preferred) = spec.options.selected_reasoning_mode() {
                builder = builder.reasoning_mode(
                    spec.model
                        .capabilities(ModelTransport::Native)
                        .normalize_reasoning_mode(preferred),
                );
            }
            {
                #[cfg(not(target_family = "wasm"))]
                let providers = settings.providers;
                let tools = settings
                    .tools
                    .unwrap_or_else(|| AgentTools::Shared(nanocode_tool_host::Tools::default()));
                let process_store = spec
                    .durable_state
                    .as_ref()
                    .map(|state| state.shared_store());
                builder = builder.tools_factory(move |handle| {
                    let mut selected = tools.materialize(handle.clone())?;
                    selected = services.prepare_tools(&handle, selected, process_store.clone())?;
                    #[cfg(not(target_family = "wasm"))]
                    if !selected.has_code_mode_host()
                        && selected.workspace_enabled()
                        && selected.workspace_source().is_none()
                    {
                        selected = selected.with_workspace_source(
                            crate::builtin_tools::WorkspaceTools::with_process_executors(
                                workspace.to_path_buf(),
                                services.process_executors(),
                            )
                            .into_source(),
                        );
                    }
                    use nanocode_tools::ToolCollection as _;
                    selected = selected.add_shared_tools(
                        subagents
                            .as_ref()
                            .map(|subagents| {
                                subagents.tools(
                                    handle,
                                    nanocode_agent_config::configuration::DelegationAuthority::Session,
                                )
                            })
                            .into_iter()
                            .flatten()
                            .chain(shared.iter().cloned()),
                    )?;
                    let builder = selected.into_builder();
                    let builder = match direct && !remote {
                        true => builder.exposure(nanocode_tool_host::ToolExposure::DirectOnly),
                        false => builder,
                    };
                    #[cfg(not(target_family = "wasm"))]
                    let builder = providers.iter().fold(builder, |builder, provider| {
                        builder.shared_provider(provider.clone())
                    });
                    builder.build()
                });
            }
            if spec.options.stateless_http {
                builder = builder.stateless_http();
            }
            if let Some(checkpoint) = spec.checkpoint {
                builder = builder.resume(checkpoint)?;
            }

            if let Some(state) = spec.durable_state {
                use nanocode_durability::DurableAgentExt as _;
                let session_id = state.state_id().parse().map_err(|error| {
                    NanocodeError::InvalidRequest(format!(
                        "invalid durable Codex session identity: {error}"
                    ))
                })?;
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
                    let (deferred, existing, _seed) = builder
                        .clone()
                        .session_id(session_id)
                        .deferred_durability(&state, record, text.clone());
                    let built =
                        tracing::info_span!("codex.session.build").in_scope(|| deferred.build());
                    match nanocode_agent_core::Agent::persist_created(built).await {
                        Ok((agent, events)) => {
                            host.initial_notice_committed(&agent, text);
                            return Ok((agent, events));
                        }
                        Err(_) if existing.load(std::sync::atomic::Ordering::SeqCst) => {}
                        Err(error) => return Err(error),
                    }
                }
                builder = tracing::Instrument::instrument(
                    builder.session_id(session_id).durability(state),
                    tracing::info_span!("codex.session.durability"),
                )
                .await?;
            }
            tracing::info_span!("codex.session.build").in_scope(|| builder.build())
        })
    }
}

impl nanocode_agent_config::configuration::DefaultRuntimeBackend for CodexHarness {
    fn backend_id() -> BackendId {
        BackendId::new("codex")
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
        Arc::new(crate::durability::CodexCheckpointFormat)
    }
}
