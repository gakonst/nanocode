mod turns;

pub use turns::{DurableOutcome, PromptRejection, admit_prompt};

use crate::{
    Agent, AgentEvents, HarnessFamily, ModelSelection, NanocodeError, SessionCheckpoint, Thinking,
    router::{AgentResult, Harness, HarnessRequest},
};
use nanocode_agent_core::AgentHandle;
use nanocode_agent_core::{
    ModelTransport, SpawnOptions, WeakAgent,
    backend::{AgentFactory, AgentOwner, BackendFuture, adopt},
};
use std::{
    collections::HashMap,
    future::Future,
    marker::PhantomData,
    path::{Path, PathBuf},
    pin::pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use nanocode_sync::{Atom, Inbox, Swap, SwapOption};

type Result<T> = nanocode_agent_core::Result<T>;

use crate::configuration::*;
use crate::durability::DurableBackendRegistration as RegisteredBackend;

type Recipe = Arc<dyn Fn(AgentSpec) -> BackendFuture<AgentResult> + Send + Sync>;

pub type SubagentUpdates =
    tokio::sync::mpsc::UnboundedReceiver<nanocode_subagents::ScopedAgentUpdate>;
type UpdateSlot = Arc<SwapOption<SubagentUpdates>>;
#[cfg(not(target_family = "wasm"))]
type RunnerFuture<'a, H, T> = nanocode_tool_host::embedded::HostFuture<
    'a,
    std::result::Result<T, <H as RunnerServices>::Error>,
>;

fn take_updates(slot: &UpdateSlot) -> Option<SubagentUpdates> {
    slot.swap(None)
        .and_then(|updates| Arc::try_unwrap(updates).ok())
}

macro_rules! settings_methods {
    () => {
        #[must_use]
        pub fn instructions(mut self, instructions: impl Into<Arc<str>>) -> Self {
            self.data.settings.instructions = Some(instructions.into());
            self
        }
    };
}

macro_rules! tool_methods {
    () => {
        #[must_use]
        pub fn tools(mut self, tools: nanocode_tool_host::Tools) -> Self {
            self.data.settings.tools = Some(AgentTools::Shared(tools));
            self
        }

        #[must_use]
        pub fn tools_factory<T>(mut self, factory: T) -> Self
        where
            T: Fn(
                    AgentHandle,
                ) -> std::result::Result<
                    nanocode_tool_host::Tools,
                    nanocode_tool_host::ToolsBuildError,
                > + Send
                + Sync
                + 'static,
        {
            self.data.settings.tools = Some(AgentTools::PerAgent(Arc::new(factory)));
            self
        }
    };
}

#[derive(Clone, Default)]
pub struct Unselected;

#[derive(Clone, Default)]
pub struct RuntimeBuilder<B, D> {
    selection: PhantomData<fn() -> B>,
    defaults: D,
    data: Assembly,
}

#[derive(Clone, Default)]
struct Assembly {
    backends: Vec<Arc<dyn RuntimeBackend>>,
    durability:
        Option<std::result::Result<crate::durability::SessionStore, Arc<crate::durability::Error>>>,
    harness: Option<HarnessFamily>,
    model: Option<ModelSelection>,
    thinking: Option<Thinking>,
    settings: Settings,
    workspace: Option<PathBuf>,
    max_subagents: Option<usize>,
    subagents_disabled: bool,
    subagent_continuation_disabled: bool,
}

impl<B, D: RuntimeDefaults> RuntimeBuilder<B, D> {
    tool_methods!();

    pub fn code_mode_host(
        mut self,
        host: impl nanocode_tool_host::embedded::CodeModeHost,
    ) -> std::result::Result<Self, nanocode_tool_host::ToolsBuildError> {
        let host = Arc::new(host);
        let with_host = move |tools: nanocode_tool_host::Tools| {
            tools
                .into_builder()
                .without_defaults()
                .code_mode_host(Arc::clone(&host))
                .build()
        };
        self.data.settings.tools = Some(match self.data.settings.tools.take() {
            Some(AgentTools::Shared(tools)) => AgentTools::Shared(with_host(tools)?),
            Some(AgentTools::PerAgent(factory)) => {
                AgentTools::PerAgent(Arc::new(move |handle| with_host(factory(handle)?)))
            }
            None => AgentTools::Shared(with_host(nanocode_tool_host::Tools::default())?),
        });
        #[cfg(not(target_family = "wasm"))]
        {
            self.defaults.disable_machine_tools();
        }
        Ok(self)
    }

    #[must_use]
    pub fn backend(mut self, backend: impl RegisteredBackend + 'static) -> Self {
        self.data
            .backends
            .push(Arc::new(Registered(Arc::new(backend))));
        self
    }

    #[must_use]
    pub fn durability(mut self, store: impl crate::durability::IntoSessionStore) -> Self {
        self.data.durability = Some(store.into_session_store().map_err(Arc::new));
        self
    }

    #[must_use]
    pub const fn max_subagents(mut self, limit: usize) -> Self {
        self.data.max_subagents = Some(limit);
        self.data.subagents_disabled = false;
        self
    }

    #[must_use]
    pub const fn without_subagents(mut self) -> Self {
        self.data.subagents_disabled = true;
        self
    }

    #[must_use]
    pub const fn without_subagent_continuation(mut self) -> Self {
        self.data.subagent_continuation_disabled = true;
        self
    }

    #[must_use]
    pub fn openai(self, harness: impl Into<D::Codex>) -> Self {
        self.install_harness(Arc::new(harness.into()))
    }

    #[must_use]
    pub fn claude(self, harness: impl Into<D::Claude>) -> Self {
        self.install_harness(Arc::new(harness.into()))
    }

    fn install_harness(mut self, harness: Arc<dyn RuntimeBackend>) -> Self {
        let id = harness.id();
        self.data
            .backends
            .retain(|backend| !(backend.is_harness() && backend.id() == id));
        self.data.backends.push(harness);
        self
    }

    #[must_use]
    pub const fn thinking(mut self, thinking: Thinking) -> Self {
        self.data.thinking = Some(thinking);
        self
    }

    settings_methods!();

    #[must_use]
    pub fn model_instructions(
        mut self,
        model: ModelSelection,
        instructions: impl Into<Arc<str>>,
    ) -> Self {
        let model = model
            .as_codex()
            .map(ModelSelection::Codex)
            .or_else(|| model.as_claude().map(ModelSelection::Claude))
            .unwrap_or(model);
        let configured = &mut self.data.settings.model_instructions;
        configured.retain(|(existing, _)| *existing != model);
        configured.push((model, instructions.into()));
        self
    }

    #[must_use]
    pub fn images(mut self, images: nanocode_tool_host::image::ImageHistory) -> Self {
        self.data.settings.images = Some(images);
        self
    }

    #[must_use]
    pub fn workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.data.workspace = Some(workspace.into());
        self
    }

    pub fn build(mut self) -> Result<Runtime<D>> {
        let harnesses = self
            .data
            .backends
            .iter()
            .filter(|backend| backend.is_harness())
            .map(|backend| backend.id())
            .collect::<Vec<_>>();
        let preferred = D::preferred_backend(&harnesses)
            .or_else(|| self.data.backends.first().map(|backend| backend.id()))
            .map(HarnessFamily::from);
        let existing = self
            .data
            .backends
            .iter()
            .map(|backend| backend.id())
            .collect::<Vec<_>>();
        self.data.backends.extend(D::default_backends(&existing));
        if self.data.durability.is_none() {
            self.data.durability = Some(D::default_store().map_err(Arc::new));
        }
        let host = self.defaults.build()?;
        let services: Arc<dyn HostServices> = host.clone();
        let inner = assemble(self.data, preferred, services)?;
        Ok(Runtime { inner, host })
    }

    pub async fn build_async(self) -> Result<Runtime<D>>
    where
        B: 'static,
    {
        #[cfg(not(target_family = "wasm"))]
        {
            let runtime = tokio::runtime::Handle::try_current()
                .map_err(|_| NanocodeError::TokioRuntimeUnavailable)?;
            let mut builder = self;
            if builder.data.durability.is_none()
                && let Some(store) = D::default_store_async()
            {
                builder.data.durability = Some(store.await.map_err(Arc::new));
            }
            let span = tracing::Span::current();
            let (completed, result) = tokio::sync::oneshot::channel();
            std::thread::Builder::new()
                .name("nanocode-runtime-construction".into())
                .spawn(move || {
                    if completed.is_closed() {
                        return;
                    }
                    let _runtime = runtime.enter();
                    let _ = completed.send(span.in_scope(|| builder.build()));
                })
                .map_err(|error| NanocodeError::backend("runtime construction", error))?;
            result
                .await
                .map_err(|error| NanocodeError::backend("runtime construction", error))?
        }
        #[cfg(target_family = "wasm")]
        {
            self.build()
        }
    }
}

fn assemble(
    data: Assembly,
    preferred: Option<HarnessFamily>,
    services: Arc<dyn HostServices>,
) -> Result<AgentRuntime> {
    let mut configured = Vec::new();

    for backend in &data.backends {
        let family: HarnessFamily = backend.id().as_str().parse().map_err(|error| {
            NanocodeError::InvalidRequest(format!("invalid backend identity: {error}"))
        })?;
        if family.as_str().trim().is_empty() {
            return Err(NanocodeError::InvalidRequest(
                "backend identity must not be empty".into(),
            ));
        }
        backend.validate_model(backend.default_model().as_str())?;
        if configured.contains(&family) {
            return Err(NanocodeError::InvalidRequest(format!(
                "backend {family} is configured more than once"
            )));
        }
        configured.push(family);
    }
    let Some(fallback) = preferred
        .filter(|family| configured.contains(family))
        .or_else(|| configured.first().cloned())
    else {
        return Err(NanocodeError::InvalidRequest(
            "a Nanocode runtime requires at least one harness backend".into(),
        ));
    };
    if let (Some(family), Some(model)) = (data.harness.as_ref(), data.model.as_ref())
        && *family != model.family()
    {
        return Err(NanocodeError::InvalidRequest(
            "model does not belong to selected harness".into(),
        ));
    }
    let family = data
        .harness
        .or_else(|| data.model.as_ref().map(ModelSelection::family))
        .unwrap_or(fallback);
    let model = data.model.unwrap_or_else(|| {
        data.backends
            .iter()
            .find(|backend| backend.id().as_str() == family.as_str())
            .map_or_else(
                || family.default_model(),
                |backend| ModelSelection::new(family.clone(), backend.default_model().as_str()),
            )
    });
    model.validate()?;
    if let Some(backend) = data
        .backends
        .iter()
        .find(|backend| backend.id().as_str() == family.as_str())
    {
        backend.validate_model(model.as_str())?;
    }
    let thinking = data.thinking.unwrap_or_else(|| {
        data.backends
            .iter()
            .find(|backend| {
                backend.id().as_str() == family.as_str() && backend.default_model() == model
            })
            .and_then(|backend| backend.default_thinking())
            .unwrap_or_else(|| model.default_thinking())
    });
    model
        .capabilities(ModelTransport::Native)
        .check_thinking(thinking)?;
    if !configured.contains(&family) {
        return Err(not_configured(family));
    }
    let workspace: Arc<Path> = resolve_workspace(data.workspace)?.into();

    let ownership = Arc::new(Ownership::default());
    let _ = ownership.services.set(Arc::downgrade(&services));
    let backend_defaults: Vec<_> = data
        .backends
        .iter()
        .map(|backend| {
            let model = backend.default_model();
            let thinking = backend
                .default_thinking()
                .unwrap_or_else(|| model.default_thinking());
            (model, thinking)
        })
        .collect();
    let durability = data
        .durability
        .transpose()
        .map_err(|error| NanocodeError::Backend {
            backend: "durability",
            source: error,
        })?;
    let mut ephemeral = Vec::new();
    if let Some(store) = &durability {
        for backend in &data.backends {
            match backend.checkpoint_format() {
                Some(format) => store.register_format(format).map_err(catalog_error)?,
                None => ephemeral.push(backend.id()),
            }
        }
    }
    let (subagents, subagent_control, subagent_updates) = if data.subagents_disabled {
        (None, None, None)
    } else {
        let (registry, control, updates) = nanocode_subagents::channel(
            data.max_subagents
                .unwrap_or(nanocode_subagents::DEFAULT_MAX_SUBAGENTS),
        );
        if !data.subagent_continuation_disabled {
            let owners = Arc::downgrade(&ownership);
            registry.set_parent_resolver(Arc::new(move |session_id: &str| {
                owners.upgrade()?.agent(session_id)
            }));
        }
        let released = Arc::downgrade(&services);
        registry.set_session_released(Arc::new(move |session_id: &str| {
            if let Some(services) = released.upgrade() {
                services.unregister(session_id);
            }
        }));
        (Some(registry), Some(control), Some(updates))
    };
    let subagent_updates: UpdateSlot = Arc::new(SwapOption::from(subagent_updates.map(Arc::new)));
    let mut recipes: Vec<(HarnessFamily, Recipe)> = Vec::new();

    for backend in data.backends {
        let family = backend.id().as_str().parse().map_err(|error| {
            NanocodeError::InvalidRequest(format!("invalid backend identity: {error}"))
        })?;
        recipes.push((
            family,
            backend_recipe(
                backend,
                &data.settings,
                &workspace,
                services.clone(),
                subagents.clone(),
            ),
        ));
    }

    if let Some(store) = &durability {
        for (family, recipe) in &mut recipes {
            if ephemeral.iter().any(|id| id.as_str() == family.as_str()) {
                continue;
            }
            *recipe = persistent_recipe(recipe.clone(), store.clone(), workspace.clone());
        }
    }
    if subagent_control.is_some() {
        for (_, recipe) in &mut recipes {
            *recipe = unclaimed_updates_recipe(recipe.clone(), subagent_updates.clone());
        }
    }
    for (_, recipe) in &mut recipes {
        *recipe = host_recipe(recipe.clone(), services.clone());
    }
    let mut router = Harness::builder();

    if let Some(store) = &durability {
        router = router.recovery_store(store.clone());
    }
    for (family, recipe) in &recipes {
        let recipe = Arc::clone(recipe);
        router = router.register(family.clone(), move |request: HarnessRequest| {
            recipe(AgentSpec::routed(request))
        });
    }
    for (model, thinking) in backend_defaults {
        router = router.default_settings(model, thinking);
    }
    services.activate();
    let inner = Arc::new(RuntimeState {
        harness: router.build_tracked(Arc::downgrade(&ownership)),
        recipes,
        model,
        thinking,
        ownership,
        services,

        durability,
        subagents: subagent_control,
        subagent_updates,
    });
    if inner.durability.is_some() {
        let runtime = Arc::downgrade(&inner);
        let _ = inner
            .ownership
            .reopener
            .set(Arc::new(move |session_id: String| {
                let runtime = runtime.clone();
                Box::pin(async move {
                    let inner = runtime.upgrade().ok_or(NanocodeError::AgentStopped)?;
                    AgentRuntime { inner }.reconstruct(&session_id).await
                })
            }));
    }
    Ok(AgentRuntime { inner })
}

impl<D: RuntimeDefaults> RuntimeBuilder<Unselected, D> {
    #[must_use]
    pub fn harness_model(mut self, model: ModelSelection) -> Self {
        self.data.harness = Some(model.family());
        self.data.model = Some(model);
        self
    }

    #[must_use]
    pub fn for_backend<B: crate::BackendKind>(self) -> RuntimeBuilder<B, D> {
        let mut data = self.data;
        data.harness = Some(B::backend_id().into());
        data.model = None;
        RuntimeBuilder {
            selection: PhantomData,
            defaults: self.defaults,
            data,
        }
    }
}

impl<B: crate::BackendKind, D: RuntimeDefaults> RuntimeBuilder<B, D> {
    #[must_use]
    pub fn model(mut self, model: B::Model) -> Self {
        self.data.model = Some(ModelSelection::new(B::backend_id(), B::model_id(&model)));
        self
    }
}

#[cfg(not(target_family = "wasm"))]
impl<B, D: NativeDefaults> RuntimeBuilder<B, D> {
    #[must_use]
    pub fn runner_management(mut self, enabled: bool) -> Self {
        self.defaults.runner_management(enabled);
        self
    }
    #[must_use]
    pub fn without_runners(mut self) -> Self {
        self.defaults.without_runners();
        self
    }
    #[must_use]
    pub fn runner(mut self, runner: D::Runner) -> Self {
        self.defaults.runner(runner);
        self
    }
    #[must_use]
    pub fn runner_server(mut self, config: D::RunnerServerConfig) -> Self {
        self.defaults.runner_server(config);
        self
    }
    #[must_use]
    pub fn cua(mut self, tools: D::Computer) -> Self {
        self.defaults.computer(Some(tools));
        self
    }
    #[must_use]
    pub fn without_cua(mut self) -> Self {
        self.defaults.computer(None);
        self
    }
    #[must_use]
    pub fn tool_environment<K, V>(mut self, variables: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<std::ffi::OsString>,
        V: Into<std::ffi::OsString>,
    {
        self.defaults.tool_environment(
            variables
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        );
        self
    }
    #[must_use]
    pub fn tool_provider(
        mut self,
        provider: impl nanocode_tool_host::runtime::DynamicToolProvider + 'static,
    ) -> Self {
        self.data.settings.providers.push(Arc::new(provider));
        self
    }
}

fn backend_recipe(
    backend: Arc<dyn RuntimeBackend>,
    runtime: &Settings,
    workspace: &Arc<Path>,
    services: Arc<dyn HostServices>,
    subagents: Option<Arc<nanocode_subagents::Registry>>,
) -> Recipe {
    let workspace = workspace.clone();
    let defaults = runtime.clone();
    Arc::new(move |spec| {
        let mut settings = spec.settings;
        if settings.instructions.is_none() {
            settings.instructions = defaults
                .model_instructions
                .iter()
                .find(|(model, _)| *model == spec.model)
                .map(|(_, instructions)| Arc::clone(instructions));
        }
        backend.build(RuntimeRequest {
            request: HarnessRequest {
                model: spec.model,
                thinking: spec.thinking,
                options: spec.options,
                parent: spec.parent,
                host_context: spec.host_context,
                checkpoint: spec.checkpoint,
                durable_state: spec.durable_state,
                spawn_factory: spec.spawn_factory,
            },
            settings,
            defaults: defaults.clone(),
            workspace: workspace.clone(),
            services: services.clone(),
            subagents: subagents
                .clone()
                .map(|registry| Arc::new(Delegation(registry)) as Arc<dyn DelegationTools>),
        })
    })
}

struct Delegation(Arc<nanocode_subagents::Registry>);

impl DelegationTools for Delegation {
    fn tools(
        &self,
        parent: AgentHandle,
        authority: DelegationAuthority,
    ) -> Vec<Arc<dyn nanocode_tool_contracts::Tool>> {
        let Ok(SharedTools(tools)) = match authority {
            DelegationAuthority::Session => {
                nanocode_subagents::install_tools(SharedTools(Vec::new()), parent, self.0.clone())
            }
            DelegationAuthority::Turn => nanocode_subagents::install_claude_tools(
                SharedTools(Vec::new()),
                parent,
                self.0.clone(),
            ),
        };
        tools
    }
}

struct Registered(Arc<dyn RegisteredBackend>);
impl RuntimeBackend for Registered {
    fn id(&self) -> crate::BackendId {
        self.0.id()
    }
    fn default_model(&self) -> ModelSelection {
        ModelSelection::new(self.0.id(), self.0.default_model())
    }
    fn default_thinking(&self) -> Option<Thinking> {
        self.0.default_thinking()
    }
    fn validate_model(&self, model: &str) -> Result<()> {
        self.0.validate_model(model)
    }
    fn checkpoint_format(&self) -> Option<Arc<dyn crate::durability::CheckpointFormat>> {
        self.0.checkpoint_format()
    }
    fn build(&self, request: RuntimeRequest) -> BackendFuture<AgentResult> {
        let backend = self.0.clone();
        Box::pin(async move {
            let RuntimeRequest {
                request: spec,
                settings,
                defaults,
                workspace,
                subagents,
                services,
            } = request;
            let settings = settings.or(&defaults);
            backend.validate_model(spec.model.as_str())?;
            if settings.tools.is_some() {
                return Err(NanocodeError::InvalidRequest("registered backends cannot receive facade tool selections; configure tools on the backend or use the Codex or Claude harness".into()));
            }
            let mut tools = nanocode_agent_core::RuntimeTools::default()
                .shared(services.shared_tools(false).await?);
            if let Some(registry) = subagents {
                tools = tools.per_agent(Arc::new(move |handle| {
                    registry.tools(handle.clone(), DelegationAuthority::Session)
                }));
            }
            let request = crate::BackendRequest {
                tools,
                model: spec.model,
                thinking: Some(spec.thinking),
                options: spec.options,
                workspace: workspace.to_path_buf(),
                instructions: settings.instructions.map(|value| value.to_string()),
                spawn_factory: spec.spawn_factory,
                host_context: spec.host_context,
                checkpoint: spec.checkpoint,
            };
            match spec.durable_state {
                Some(state) => backend.build_durable(request, state).await,
                None => backend.build(request).await,
            }
        })
    }
}
fn unclaimed_updates_recipe(recipe: Recipe, updates: UpdateSlot) -> Recipe {
    Arc::new(move |spec| {
        let recipe = recipe.clone();
        let updates = updates.clone();
        Box::pin(async move {
            drop(take_updates(&updates));
            recipe(spec).await
        })
    })
}

fn persistent_recipe(
    recipe: Recipe,
    store: crate::durability::SessionStore,
    workspace: Arc<Path>,
) -> Recipe {
    Arc::new(move |mut spec| {
        let recipe = recipe.clone();
        let store = store.clone();
        let workspace = workspace.clone();
        Box::pin(async move {
            let mut interrupted = Vec::new();
            if spec.durable_state.is_none()
                && let Some(checkpoint) = &spec.checkpoint
            {
                match store.resumable(checkpoint.session_id()).await {
                    Ok(Some(stored)) => {
                        interrupted = stored
                            .pending
                            .iter()
                            .map(|turn| (turn.id.clone(), turn.input.clone()))
                            .collect();
                        let record = &stored.record;
                        let automatic_restore = spec.parent.is_some();
                        if record.family() != checkpoint.family()
                            || (!automatic_restore && record.model != spec.model)
                            || record.lineage != *checkpoint.lineage()
                        {
                            return Err(NanocodeError::InvalidCheckpoint(
                                "checkpoint identity differs from the stored session".into(),
                            ));
                        }
                        if spec.options.provider.is_none() {
                            spec.options.provider.clone_from(&record.provider);
                        }
                        spec.durable_state = Some(
                            store
                                .prepare_resume(checkpoint.session_id())
                                .await
                                .map_err(catalog_error)?,
                        );
                        if automatic_restore && stored.checkpointed {
                            spec.checkpoint = None;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => return Err(catalog_error(error)),
                }
            }
            if spec.durable_state.is_none() {
                let (id, lineage) = if let Some(checkpoint) = &spec.checkpoint {
                    (
                        checkpoint.session_id().to_owned(),
                        checkpoint.lineage().clone(),
                    )
                } else {
                    let id = spec
                        .session_id
                        .take()
                        .unwrap_or_else(|| nanocode_platform::uuid_v7().to_string());
                    let lineage = if let Some(parent) = &spec.parent {
                        let parent_lineage = match store.record(parent.session_id()).await {
                            Ok(Some(record)) => record.lineage,
                            Ok(None) => crate::Lineage::root(parent.session_id()),
                            Err(error) => return Err(catalog_error(error)),
                        };
                        crate::Lineage::child_of(
                            &parent_lineage,
                            parent.session_id(),
                            crate::Origin::Subagent,
                        )
                    } else {
                        crate::Lineage::root(id.clone())
                    };
                    (id, lineage)
                };
                let mut record = crate::durability::SessionRecord::root(
                    id,
                    spec.model.clone(),
                    Some(workspace.to_path_buf()),
                )
                .with_lineage(lineage);
                record.provider.clone_from(&spec.options.provider);
                spec.durable_state =
                    Some(store.prepare_session(record).await.map_err(catalog_error)?);
            }
            let durable = spec
                .durable_state
                .as_ref()
                .map(crate::durability::DurableSession::downgrade);
            let (agent, events) = recipe(spec).await?;
            let agent = match durable {
                Some(durable) => agent.with_ownership_probe(move || {
                    let durable = durable.clone();
                    Box::pin(async move {
                        durable
                            .is_current_owner()
                            .await
                            .map_err(crate::durability::agent_error)
                    })
                }),
                None => agent,
            };
            Ok((agent.with_interrupted_turns(interrupted), events))
        })
    })
}

#[derive(Clone)]
pub struct AgentRuntime {
    inner: Arc<RuntimeState>,
}

struct RuntimeState {
    harness: Harness,
    recipes: Vec<(HarnessFamily, Recipe)>,
    model: ModelSelection,
    thinking: Thinking,
    ownership: Arc<Ownership>,
    services: Arc<dyn HostServices>,

    durability: Option<crate::durability::SessionStore>,
    subagents: Option<nanocode_subagents::SubagentControl>,
    subagent_updates: UpdateSlot,
}

impl AgentRuntime {
    pub fn spawn(&self) -> SpawnBuilder<'_> {
        SpawnBuilder {
            selection: PhantomData,
            runtime: self,
            options: SpawnOptions::new(),
            settings: Settings::default(),
            session_id: None,
        }
    }

    #[must_use]
    pub fn harness(&self) -> HarnessFamily {
        self.inner.model.family()
    }

    #[must_use]
    pub fn model(&self) -> ModelSelection {
        self.inner.model.clone()
    }

    #[must_use]
    pub fn thinking(&self) -> Thinking {
        self.inner.thinking
    }

    #[must_use]
    pub fn computer_use_unavailable(&self) -> Option<String> {
        self.inner.services.unavailable()
    }

    #[must_use]
    pub fn is_configured(&self, family: HarnessFamily) -> bool {
        self.inner.recipe(family).is_ok()
    }

    #[must_use]
    pub fn spawn_factory(&self) -> Arc<dyn AgentFactory> {
        self.inner.harness.spawn_factory()
    }

    #[must_use]
    pub fn subagents(&self) -> Option<&nanocode_subagents::SubagentControl> {
        self.inner.subagents.as_ref()
    }

    #[must_use]
    pub fn take_subagent_updates(&self) -> Option<SubagentUpdates> {
        take_updates(&self.inner.subagent_updates)
    }

    pub async fn resume(&self, checkpoint: SessionCheckpoint) -> AgentResult {
        checkpoint.validate()?;
        let _opening = match &self.inner.durability {
            Some(_) => match self
                .inner
                .ownership
                .open_session(checkpoint.session_id())
                .await
            {
                Opened::Live(agent) => return Ok(attached(agent)),
                Opened::Vacant(opening) => Some(opening),
            },
            None => None,
        };
        let model = checkpoint.model();
        let thinking = checkpoint.thinking();
        self.construct(AgentSpec {
            parent: None,
            options: SpawnOptions::new()
                .harness_model(model.clone())
                .thinking(thinking),
            model,
            thinking,
            settings: Settings::default(),
            spawn_factory: self.spawn_factory(),
            host_context: None,
            checkpoint: Some(checkpoint),

            durable_state: None,
            session_id: None,
        })
        .await
    }

    pub async fn open_session(&self, session_id: &str) -> AgentResult {
        let store = self.inner.durability.as_ref().ok_or_else(|| {
            NanocodeError::InvalidRequest("runtime durability is not configured".into())
        })?;
        self.open(store, session_id).await
    }

    pub async fn open(
        &self,
        store: &crate::durability::SessionStore,
        session_id: &str,
    ) -> AgentResult {
        let configured = self.configured_store(store)?;
        let opening = match self.inner.ownership.open_session(session_id).await {
            Opened::Live(agent) => return Ok(attached(agent)),
            Opened::Vacant(opening) => opening,
        };
        let stored = configured.load(session_id).await.map_err(catalog_error)?;
        self.open_stored(configured, &stored, Some(opening)).await
    }

    pub async fn open_loaded(
        &self,
        store: &crate::durability::SessionStore,
        stored: &crate::durability::StoredSession,
    ) -> AgentResult {
        let configured = self.configured_store(store)?;
        let session_id = &stored.summary.record.session_id;
        let opening = match self.inner.ownership.open_session(session_id).await {
            Opened::Live(agent) => return Ok(attached(agent)),
            Opened::Vacant(opening) => opening,
        };
        self.open_stored(configured, stored, Some(opening)).await
    }

    async fn reconstruct(&self, session_id: &str) -> AgentResult {
        let configured = self.inner.durability.as_ref().ok_or_else(|| {
            NanocodeError::InvalidRequest("runtime durability is not configured".into())
        })?;
        let _opening = self.inner.ownership.claim_opening(session_id).await;
        let stored = configured.load(session_id).await.map_err(catalog_error)?;
        self.open_stored(configured, &stored, None).await
    }

    async fn open_stored(
        &self,
        configured: &crate::durability::SessionStore,
        stored: &crate::durability::StoredSession,
        opening: Option<SessionOpening>,
    ) -> AgentResult {
        let model = stored.summary.record.model.clone();
        let thinking = stored
            .checkpoint_thinking()
            .map_err(catalog_error)?
            .unwrap_or_else(|| {
                stored
                    .summary
                    .record
                    .initial
                    .map_or_else(|| model.default_thinking(), |initial| initial.thinking)
            });
        self.inner.recipe(model.family())?;
        let inherited = stored
            .turns
            .iter()
            .all(|turn| turn.status != crate::durability::TurnStatus::Pending)
            .then(|| stored.summary.record.inherited_transcript.clone())
            .filter(|transcript| !transcript.is_empty());
        let durable_state = configured
            .prepare_resume_loaded(stored)
            .await
            .map_err(catalog_error)?;
        let _preload = CheckpointPreload(durable_state.clone());
        let _journal = match &self.inner.subagents {
            Some(control) => JournalReservation::new(control.registry(), &durable_state)
                .map_err(catalog_error)?,
            None => None,
        };
        let mut options = SpawnOptions::new()
            .harness_model(model.clone())
            .thinking(thinking);
        options.provider.clone_from(&stored.summary.record.provider);
        let spec = AgentSpec {
            parent: None,
            options,
            model,
            thinking,
            settings: Settings::default(),
            spawn_factory: self.spawn_factory(),
            host_context: None,
            checkpoint: None,
            durable_state: Some(durable_state),
            session_id: None,
        };
        let (agent, events) = match opening {
            Some(_) => self.construct(spec).await?,
            None => self.inner.recipe(spec.model.family())?(spec).await?,
        };
        let agent = agent.with_interrupted_turns(
            stored
                .turns
                .iter()
                .filter(|turn| turn.status == crate::durability::TurnStatus::Pending)
                .map(|turn| (turn.id.clone(), turn.input.clone()))
                .collect(),
        );
        match inherited {
            Some(transcript) => Ok((agent.with_transcript(transcript)?, events)),
            None => Ok((agent, events)),
        }
    }

    fn configured_store(
        &self,
        store: &crate::durability::SessionStore,
    ) -> Result<&crate::durability::SessionStore> {
        let configured = self.inner.durability.as_ref().ok_or_else(|| {
            NanocodeError::InvalidRequest("runtime durability is not configured".into())
        })?;
        if !configured.same_store(store) {
            return Err(NanocodeError::InvalidRequest(
                "session store differs from the runtime's configured durability store".into(),
            ));
        }
        Ok(configured)
    }

    pub async fn switch(
        &self,
        agent: &Agent,
        model: ModelSelection,
        thinking: Option<Thinking>,
    ) -> AgentResult {
        model.validate()?;
        let thinking = thinking.unwrap_or_else(|| model.default_thinking());
        model
            .capabilities(ModelTransport::Native)
            .check_thinking(thinking)?;
        self.inner.recipe(model.family())?;
        if model.family() == agent.harness_family()
            && agent.capabilities().checkpoint
            && agent.checkpoint().await?.model() == model
        {
            match agent.fork(crate::ForkRequest::latest()).await {
                Ok((switched, events)) => {
                    let configured = async {
                        switched.set_thinking(thinking).await?;
                        self.inner.services.register(&switched, true).await?;
                        Ok(())
                    }
                    .await;
                    return match configured {
                        Ok(()) => Ok((switched, events)),
                        Err(error) => {
                            let _ = switched.shutdown().await;
                            Err(error)
                        }
                    };
                }
                Err(NanocodeError::ForkBeforeCompletedTurn) => {}
                Err(error) => return Err(error),
            }
        }
        let transcript = agent.transcript().await?;
        let mut options = SpawnOptions::new()
            .harness_model(model.clone())
            .thinking(thinking);
        let durable_state = match &self.inner.durability {
            Some(store) => {
                let parent = agent.session_id();
                let (lineage, workspace) = match store.load(parent).await {
                    Ok(stored) => {
                        options.provider = stored.summary.record.provider;
                        (
                            stored.summary.record.lineage,
                            stored.summary.record.workspace,
                        )
                    }
                    Err(crate::durability::Error::SessionNotFound { .. }) => {
                        (crate::Lineage::root(parent), None)
                    }
                    Err(error) => return Err(catalog_error(error)),
                };
                let mut record = crate::durability::SessionRecord::root(
                    nanocode_platform::uuid_v7().to_string(),
                    model.clone(),
                    workspace,
                )
                .with_lineage(crate::Lineage::child_of(
                    &lineage,
                    parent,
                    crate::Origin::Fork,
                ))
                .with_inherited_transcript(transcript.clone());
                record.provider.clone_from(&options.provider);
                Some(store.prepare_session(record).await.map_err(catalog_error)?)
            }
            None => None,
        };
        let (switched, events) = self
            .construct(AgentSpec {
                parent: None,
                options,
                model,
                thinking,
                settings: Settings::default(),
                spawn_factory: self.spawn_factory(),
                host_context: None,
                checkpoint: None,
                durable_state,
                session_id: None,
            })
            .await?;
        Ok((switched.with_transcript(transcript)?, events))
    }

    pub async fn shutdown(&self) -> Result<()> {
        if let Some(subagents) = &self.inner.subagents {
            subagents.shutdown().await;
        }
        self.inner.services.close();
        let host_result = self.inner.services.shutdown().await;
        let ownership_result = self.inner.ownership.shutdown().await;
        let store_result = match &self.inner.durability {
            Some(store) => store.flush().await.map_err(catalog_error),
            None => Ok(()),
        };
        host_result?;
        ownership_result?;
        store_result
    }

    async fn construct(&self, spec: AgentSpec) -> AgentResult {
        let recipe = self.inner.recipe(spec.model.family())?;
        let ownership = Arc::clone(&self.inner.ownership);
        let span = tracing::info_span!(
            "agent.session.create",
            model = %spec.model,
            resume = spec.checkpoint.is_some() || spec.durable_state.is_some()
        );
        tracing::Instrument::instrument(ownership.own(move || recipe(spec)), span).await
    }
}

impl RuntimeState {
    fn recipe(&self, family: HarnessFamily) -> Result<Recipe> {
        self.recipes
            .iter()
            .find(|(configured, _)| *configured == family)
            .map(|(_, recipe)| Arc::clone(recipe))
            .ok_or_else(|| not_configured(family))
    }
}

#[must_use = "call build().await to construct the agent"]
pub struct SpawnBuilder<'a, B = Unselected> {
    selection: PhantomData<fn() -> B>,
    runtime: &'a AgentRuntime,
    options: SpawnOptions,
    settings: Settings,
    session_id: Option<String>,
}

impl<B> SpawnBuilder<'_, B> {
    pub fn reasoning_mode(mut self, reasoning_mode: crate::ReasoningMode) -> Self {
        self.options = self.options.reasoning_mode(reasoning_mode);
        self
    }

    pub fn thinking(mut self, thinking: Thinking) -> Self {
        self.options = self.options.thinking(thinking);
        self
    }

    pub fn instructions(mut self, instructions: impl Into<Arc<str>>) -> Self {
        self.settings.instructions = Some(instructions.into());
        self
    }

    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn provider(mut self, provider: impl Into<String>) -> Self {
        self.options = self.options.provider(provider);
        self
    }

    pub async fn build(self) -> AgentResult {
        let runtime = &self.runtime.inner;
        if self.session_id.is_some() && runtime.durability.is_none() {
            return Err(NanocodeError::InvalidRequest(
                "a spawned session ID names a durable session, and runtime durability is not configured".into(),
            ));
        }
        let resolved = runtime.harness.resolve_options(
            self.options,
            runtime.model.clone(),
            runtime.thinking,
        )?;
        let model = resolved
            .selected_harness_model()
            .unwrap_or_else(|| runtime.model.clone());
        let thinking = resolved.selected_thinking().unwrap_or(runtime.thinking);
        self.runtime
            .construct(AgentSpec {
                parent: None,
                options: resolved,
                model,
                thinking,
                settings: self.settings,
                spawn_factory: self.runtime.spawn_factory(),
                host_context: None,
                checkpoint: None,

                durable_state: None,
                session_id: self.session_id,
            })
            .await
    }
}

impl<'a> SpawnBuilder<'a, Unselected> {
    pub fn harness_model(mut self, model: ModelSelection) -> Self {
        self.options = self.options.harness(model.family()).harness_model(model);
        self
    }

    pub fn backend<B: crate::BackendKind>(self) -> SpawnBuilder<'a, B> {
        SpawnBuilder {
            selection: PhantomData,
            runtime: self.runtime,
            options: self.options.harness(B::backend_id().into()),
            settings: self.settings,
            session_id: self.session_id,
        }
    }
}

impl<B: crate::BackendKind> SpawnBuilder<'_, B> {
    pub fn model(mut self, model: B::Model) -> Self {
        self.options = self
            .options
            .harness_model(ModelSelection::new(B::backend_id(), B::model_id(&model)));
        self
    }
}

struct CheckpointPreload(crate::durability::DurableSession);

struct JournalReservation {
    registry: Arc<nanocode_subagents::Registry>,
    root: String,
    journal: nanocode_agent_core::backend::ChildJournal,
    _seed: crate::durability::JournalSeed,
}

impl JournalReservation {
    fn new(
        registry: &Arc<nanocode_subagents::Registry>,
        state: &crate::durability::DurableSession,
    ) -> crate::durability::Result<Option<Self>> {
        use crate::durability::StateStore as _;
        if !state.shared_store().batches_companion_acquisition() {
            return Ok(None);
        }
        let root = state.state_id().to_owned();
        let (journal, seed) = state.seeded_child_journal();
        if !registry.reserve_journal(&root, journal.clone()) {
            return Ok(None);
        }
        let reservation = Self {
            registry: Arc::clone(registry),
            root,
            journal,
            _seed: seed,
        };
        state.adopt_child_journal(&reservation._seed)?;
        Ok(Some(reservation))
    }
}

impl Drop for JournalReservation {
    fn drop(&mut self) {
        self.registry.release_journal(&self.root, &self.journal);
    }
}

impl Drop for CheckpointPreload {
    fn drop(&mut self) {
        drop(self.0.take_checkpoint_preload(""));
    }
}

struct AgentSpec {
    model: ModelSelection,
    thinking: Thinking,
    options: SpawnOptions,

    parent: Option<AgentHandle>,
    settings: Settings,
    spawn_factory: Arc<dyn AgentFactory>,
    host_context: Option<Arc<str>>,
    checkpoint: Option<SessionCheckpoint>,

    durable_state: Option<crate::durability::DurableSession>,
    session_id: Option<String>,
}

impl AgentSpec {
    fn routed(request: HarnessRequest) -> Self {
        Self {
            model: request.model,
            thinking: request.thinking,
            options: request.options,

            parent: request.parent,
            settings: Settings::default(),
            spawn_factory: request.spawn_factory,
            host_context: request.host_context,
            checkpoint: request.checkpoint,

            durable_state: request.durable_state,
            session_id: None,
        }
    }
}

struct SharedTools(Vec<Arc<dyn nanocode_tool_contracts::Tool>>);

impl nanocode_tool_contracts::ToolCollection for SharedTools {
    type Error = std::convert::Infallible;

    fn add_shared_tool(
        mut self,
        tool: Arc<dyn nanocode_tool_contracts::Tool>,
    ) -> std::result::Result<Self, Self::Error> {
        self.0.push(tool);
        Ok(self)
    }
}

#[cfg(target_family = "wasm")]
fn resolve_workspace(workspace: Option<PathBuf>) -> Result<PathBuf> {
    workspace
        .filter(|workspace| workspace.has_root())
        .ok_or_else(|| {
            NanocodeError::InvalidRequest(
                "a WASM runtime requires an absolute workspace path".into(),
            )
        })
}

#[cfg(not(target_family = "wasm"))]
fn resolve_workspace(workspace: Option<PathBuf>) -> Result<PathBuf> {
    match workspace {
        Some(workspace) if workspace.is_absolute() => Ok(workspace),
        workspace => {
            let current = std::env::current_dir().map_err(|error| {
                NanocodeError::InvalidRequest(format!(
                    "the runtime workspace defaults to the current directory, which is unavailable: {error}"
                ))
            })?;
            Ok(workspace.map_or_else(|| current.clone(), |relative| current.join(relative)))
        }
    }
}

fn not_configured(family: HarnessFamily) -> NanocodeError {
    NanocodeError::InvalidRequest(format!(
        "{family} harness is not configured for this runtime"
    ))
}

fn attached(agent: Agent) -> (Agent, AgentEvents) {
    let events = agent.subscribe();
    (agent, events)
}

pub(crate) fn catalog_error(error: crate::durability::Error) -> NanocodeError {
    match error {
        crate::durability::Error::SessionNotFound { .. }
        | crate::durability::Error::SessionExists { .. } => {
            NanocodeError::InvalidRequest(error.to_string())
        }
        error => NanocodeError::Backend {
            backend: "durability",
            source: Arc::new(error),
        },
    }
}

const CLOSED: usize = 1 << (usize::BITS - 1);

type Reopener = Arc<dyn Fn(String) -> BackendFuture<AgentResult> + Send + Sync>;

#[derive(Default)]
pub(crate) struct Ownership {
    admissions: AtomicUsize,
    drained: AtomicBool,
    owned: Atom<nanocode_sync::Map<usize, (usize, WeakAgent)>>,
    adopted: AtomicUsize,
    children: Inbox<WeakAgent>,
    tracked: AtomicUsize,
    opening: Swap<HashMap<String, Arc<Opening>>>,
    services: OnceLock<std::sync::Weak<dyn HostServices>>,
    reopener: OnceLock<Reopener>,
    idle: tokio::sync::Notify,
    stopped: tokio::sync::OnceCell<Option<Arc<ShutdownError>>>,
}

struct Opening {
    finish: SwapOption<tokio::sync::oneshot::Sender<()>>,
    finished: futures_util::future::Shared<tokio::sync::oneshot::Receiver<()>>,
}

impl Opening {
    fn new() -> Arc<Self> {
        let (finish, finished) = tokio::sync::oneshot::channel();
        Arc::new(Self {
            finish: SwapOption::from_pointee(finish),
            finished: futures_util::FutureExt::shared(finished),
        })
    }
}

struct SessionOpening {
    ownership: Arc<Ownership>,
    session_id: String,
    opening: Arc<Opening>,
}

impl Drop for SessionOpening {
    fn drop(&mut self) {
        self.ownership.opening.rcu(|opening| {
            let mut opening = HashMap::clone(opening);
            if opening
                .get(&self.session_id)
                .is_some_and(|current| Arc::ptr_eq(current, &self.opening))
            {
                opening.remove(&self.session_id);
            }
            opening
        });
        drop(self.opening.finish.swap(None));
    }
}

enum Opened {
    Live(Agent),
    Vacant(SessionOpening),
}

struct Unstopped<'a> {
    children: &'a Inbox<WeakAgent>,
    left: Vec<WeakAgent>,
}

impl Drop for Unstopped<'_> {
    fn drop(&mut self) {
        for child in self.left.drain(..) {
            self.children.push(child);
        }
    }
}

struct Admission<'a>(&'a Ownership);

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

impl Ownership {
    fn services(&self) -> Option<Arc<dyn HostServices>> {
        self.services.get().and_then(std::sync::Weak::upgrade)
    }

    fn agent(&self, session_id: &str) -> Option<Agent> {
        self.owned.read(|owned| {
            owned
                .values()
                .filter(|(_, agent)| agent.session_id() == session_id)
                .filter_map(|(adopted, agent)| Some((adopted, agent.upgrade()?)))
                .min_by_key(|(adopted, _)| **adopted)
                .map(|(_, agent)| agent)
        })
    }

    async fn claim_opening(self: &Arc<Self>, session_id: &str) -> SessionOpening {
        loop {
            let opening = Opening::new();
            let previous = self.opening.rcu(|current| {
                let mut next = HashMap::clone(current);
                next.entry(session_id.to_owned())
                    .or_insert_with(|| Arc::clone(&opening));
                next
            });
            if let Some(concurrent) = previous.get(session_id) {
                let _ = concurrent.finished.clone().await;
                continue;
            }
            return SessionOpening {
                ownership: Arc::clone(self),
                session_id: session_id.to_owned(),
                opening,
            };
        }
    }

    async fn open_session(self: &Arc<Self>, session_id: &str) -> Opened {
        let opening = self.claim_opening(session_id).await;
        let Some(live) = self.agent(session_id) else {
            return Opened::Vacant(opening);
        };
        drop(opening);
        if (live.is_stopped() || !live.owns_session().await.unwrap_or(true))
            && let Err(error) = live.reopen().await
        {
            tracing::warn!(
                session.id = session_id,
                %error,
                "the session's agent no longer owns it and could not reopen; its next prompt retries"
            );
        }
        Opened::Live(live)
    }

    fn admission(&self) -> Result<Admission<'_>> {
        self.admit()?;
        Ok(Admission(self))
    }

    async fn own<F, Fut>(self: Arc<Self>, construct: F) -> AgentResult
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = AgentResult>,
    {
        let admission = self.admission()?;
        let (agent, events) = construct().await?;
        let owner: Arc<dyn AgentOwner> = self.clone();
        let agent = adopt(agent, &owner).await?;
        drop(admission);
        Ok((agent, events))
    }

    pub(crate) async fn track_children<F>(&self, construct: F) -> Result<Vec<(Agent, AgentEvents)>>
    where
        F: Future<Output = Result<Vec<(Agent, AgentEvents)>>>,
    {
        let admission = self.admission()?;
        let created = construct.await?;
        for (agent, _) in &created {
            self.children.push(agent.downgrade());
        }
        let before = self.tracked.fetch_add(created.len(), Ordering::AcqRel);
        if (before + created.len()).checked_ilog2() != before.checked_ilog2() {
            let tracked = self.children.drain();
            let known = tracked.len();
            let mut live = 0;
            for child in tracked.into_iter().filter(WeakAgent::is_live) {
                self.children.push(child);
                live += 1;
            }
            self.tracked.fetch_sub(known - live, Ordering::AcqRel);
        }
        if let Some(runners) = self.services() {
            for (agent, _) in &created {
                runners.register(agent, false).await?;
            }
        }
        drop(admission);
        Ok(created)
    }

    async fn shutdown(&self) -> Result<()> {
        let outcome = self
            .stopped
            .get_or_init(|| async {
                self.admissions.fetch_or(CLOSED, Ordering::AcqRel);
                loop {
                    let mut idle = pin!(self.idle.notified());
                    idle.as_mut().enable();
                    if self.admissions.load(Ordering::Acquire) & !CLOSED == 0 {
                        break;
                    }
                    idle.await;
                }
                self.drained.store(true, Ordering::Release);
                let mut failures = Vec::new();
                loop {
                    let next = self.owned.read(|owned| {
                        owned
                            .iter()
                            .max_by_key(|(_, (adopted, _))| *adopted)
                            .map(|(key, (_, agent))| (*key, agent.clone()))
                    });
                    let Some((key, agent)) = next else { break };
                    if let Some(Err(error)) = agent.shutdown().await {
                        failures.push(error.to_string());
                    }
                    self.forget(key);
                }
                let mut unstopped = Unstopped {
                    children: &self.children,
                    left: self.children.drain(),
                };
                while let Some(child) = unstopped.left.last().cloned() {
                    if let Some(Err(error)) = child.shutdown().await {
                        failures.push(error.to_string());
                    }
                    unstopped.left.pop();
                }
                (!failures.is_empty()).then(|| Arc::new(ShutdownError(failures)))
            })
            .await;
        match outcome {
            None => Ok(()),
            Some(error) => Err(NanocodeError::Backend {
                backend: "runtime",
                source: Arc::clone(error) as Arc<dyn std::error::Error + Send + Sync>,
            }),
        }
    }

    fn forget(&self, key: usize) -> Option<String> {
        self.owned
            .update(|owned| owned.remove(&key))
            .map(|(_, agent)| agent.session_id().to_owned())
    }
}

impl AgentOwner for Ownership {
    fn admit(&self) -> Result<()> {
        let mut current = self.admissions.load(Ordering::Acquire);
        loop {
            if current & CLOSED != 0 {
                return Err(NanocodeError::AgentStopped);
            }
            match self.admissions.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    fn finish(&self) {
        if self.admissions.fetch_sub(1, Ordering::AcqRel) & !CLOSED == 1 {
            self.idle.notify_waiters();
        }
    }

    fn adopt(&self, key: usize, agent: Agent) -> bool {
        if self.drained.load(Ordering::Acquire) {
            return false;
        }
        let weak = agent.downgrade();
        let adopted = self.adopted.fetch_add(1, Ordering::AcqRel);
        self.owned.update(|owned| {
            owned.insert(key, (adopted, weak.clone()));
            if owned.len().is_power_of_two() {
                owned.retain(|_, (_, agent)| agent.is_live());
            }
        });
        if let Some(services) = self.services() {
            #[cfg(not(target_family = "wasm"))]
            tokio::spawn(async move {
                let _ = services.register(&agent, false).await;
            });
            #[cfg(target_family = "wasm")]
            nanocode_platform::spawn_local(Box::pin(async move {
                let _ = services.register(&agent, false).await;
            }));
        }
        true
    }

    fn reopen(&self, session_id: &str) -> Option<BackendFuture<AgentResult>> {
        if self.admissions.load(Ordering::Acquire) & CLOSED != 0 {
            return None;
        }
        self.reopener
            .get()
            .map(|reopen| reopen(session_id.to_owned()))
    }

    fn release(&self, key: usize) {
        if let Some(session_id) = self.forget(key)
            && let Some(runners) = self.services()
        {
            runners.unregister(&session_id);
        }
    }
}
#[derive(Debug)]
struct ShutdownError(Vec<String>);

impl std::fmt::Display for ShutdownError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} agent(s) failed to shut down: {}",
            self.0.len(),
            self.0.join("; ")
        )
    }
}

impl std::error::Error for ShutdownError {}

fn host_recipe(recipe: Recipe, runners: Arc<dyn HostServices>) -> Recipe {
    Arc::new(move |spec| {
        let recipe = recipe.clone();
        let runners = runners.clone();
        Box::pin(async move {
            let await_initial = spec.checkpoint.is_none() && spec.durable_state.is_none();
            let (agent, events) = recipe(spec).await?;
            if let Err(error) = runners.register(&agent, await_initial).await {
                let _ = agent.shutdown().await;
                return Err(error);
            }
            Ok((agent, events))
        })
    })
}

pub struct Runtime<D: RuntimeDefaults> {
    inner: AgentRuntime,
    host: Arc<D::Host>,
}

impl<D: RuntimeDefaults> Clone for Runtime<D> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            host: self.host.clone(),
        }
    }
}

impl<D: RuntimeDefaults> Runtime<D> {
    #[must_use]
    pub fn builder() -> RuntimeBuilder<Unselected, D> {
        RuntimeBuilder::default()
    }
    pub fn host(&self) -> &D::Host {
        &self.host
    }
    pub fn spawn(&self) -> SpawnBuilder<'_> {
        self.inner.spawn()
    }
    pub fn harness(&self) -> HarnessFamily {
        self.inner.harness()
    }
    pub fn model(&self) -> ModelSelection {
        self.inner.model()
    }
    pub fn thinking(&self) -> Thinking {
        self.inner.thinking()
    }
    pub fn computer_use_unavailable(&self) -> Option<String> {
        self.inner.computer_use_unavailable()
    }
    pub fn is_configured(&self, family: HarnessFamily) -> bool {
        self.inner.is_configured(family)
    }
    pub fn spawn_factory(&self) -> Arc<dyn AgentFactory> {
        self.inner.spawn_factory()
    }
    pub fn subagents(&self) -> Option<&nanocode_subagents::SubagentControl> {
        self.inner.subagents()
    }
    pub fn take_subagent_updates(&self) -> Option<SubagentUpdates> {
        self.inner.take_subagent_updates()
    }
    pub fn resume(
        &self,
        checkpoint: SessionCheckpoint,
    ) -> nanocode_tool_host::embedded::HostFuture<'_, AgentResult> {
        Box::pin(self.inner.resume(checkpoint))
    }
    pub fn open_session<'a>(
        &'a self,
        session_id: &'a str,
    ) -> nanocode_tool_host::embedded::HostFuture<'a, AgentResult> {
        Box::pin(self.inner.open_session(session_id))
    }
    pub fn open<'a>(
        &'a self,
        store: &'a crate::durability::SessionStore,
        session_id: &'a str,
    ) -> nanocode_tool_host::embedded::HostFuture<'a, AgentResult> {
        Box::pin(self.inner.open(store, session_id))
    }
    pub fn switch<'a>(
        &'a self,
        agent: &'a Agent,
        model: ModelSelection,
        thinking: Option<Thinking>,
    ) -> nanocode_tool_host::embedded::HostFuture<'a, AgentResult> {
        Box::pin(self.inner.switch(agent, model, thinking))
    }
    pub fn shutdown(&self) -> nanocode_tool_host::embedded::HostFuture<'_, Result<()>> {
        Box::pin(self.inner.shutdown())
    }
}

impl<D: RuntimeDefaults> std::ops::Deref for Runtime<D> {
    type Target = AgentRuntime;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(not(target_family = "wasm"))]
impl<D: RuntimeDefaults> Runtime<D>
where
    D::Host: RunnerServices,
{
    #[must_use]
    pub fn runner_pool(&self) -> <D::Host as RunnerServices>::Pool {
        self.host.runner_pool()
    }

    pub fn initialize_runners(
        &self,
    ) -> nanocode_tool_host::embedded::HostFuture<
        '_,
        std::result::Result<(), <D::Host as RunnerServices>::Error>,
    > {
        self.host.initialize_runners()
    }

    #[must_use]
    pub fn runner_servers(&self) -> Vec<<D::Host as RunnerServices>::Server> {
        self.host.runner_servers()
    }

    pub fn connect_runner(
        &self,
        runner: <D::Host as RunnerServices>::Runner,
    ) -> RunnerFuture<'_, D::Host, <D::Host as RunnerServices>::Handle> {
        self.host.connect_runner(runner)
    }

    pub fn listen_for_runners(
        &self,
        config: <D::Host as RunnerServices>::ServerConfig,
    ) -> RunnerFuture<'_, D::Host, <D::Host as RunnerServices>::Server> {
        self.host.listen_for_runners(config)
    }

    #[must_use]
    pub fn runner_events(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<<D::Host as RunnerServices>::Event> {
        self.host.runner_events()
    }

    #[must_use]
    pub fn list_runners(&self) -> Vec<<D::Host as RunnerServices>::Info> {
        self.host.list_runners()
    }

    pub fn disconnect_runner<'a>(
        &'a self,
        name: &'a str,
    ) -> nanocode_tool_host::embedded::HostFuture<
        'a,
        std::result::Result<(), <D::Host as RunnerServices>::Error>,
    > {
        self.host.disconnect_runner(name)
    }
}
