pub use nanocode_agent_config::HarnessRequest;
use std::{collections::HashMap, future::Future, sync::Arc};

use crate::{
    Agent, AgentEvents, HarnessFamily, ModelSelection, NanocodeError, Thinking,
    runtime::catalog_error,
};
use nanocode_agent_core::{
    AgentHandle, ForkRequest, SessionCheckpoint, SpawnOptions,
    backend::{AgentFactory, BackendFuture},
};

pub(crate) type AgentResult = nanocode_agent_core::Result<(Agent, AgentEvents)>;
type Recipe = Arc<dyn Fn(HarnessRequest) -> BackendFuture<AgentResult> + Send + Sync>;

#[derive(Clone)]
pub struct Harness<D = NoFormats> {
    formats: std::marker::PhantomData<fn() -> D>,
    inner: Arc<Router>,
}

#[derive(Default)]
pub struct HarnessBuilder<D = NoFormats> {
    formats: std::marker::PhantomData<fn() -> D>,
    recipes: HashMap<HarnessFamily, Recipe>,
    defaults: HashMap<HarnessFamily, ModelSelection>,
    thinking_defaults: HashMap<HarnessFamily, Thinking>,

    recovery_store: Option<crate::durability::SessionStore>,
}

impl<D: CheckpointFormats> HarnessBuilder<D> {
    pub(crate) fn recovery_store(mut self, store: crate::durability::SessionStore) -> Self {
        self.recovery_store = Some(store);
        self
    }

    pub fn register<F, Fut>(mut self, family: HarnessFamily, recipe: F) -> Self
    where
        F: Fn(HarnessRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AgentResult> + nanocode_platform::MaybeSend + 'static,
    {
        self.defaults
            .entry(family.clone())
            .or_insert_with(|| family.default_model());
        self.recipes
            .insert(family, Arc::new(move |request| Box::pin(recipe(request))));
        self
    }

    pub fn default_model(mut self, model: ModelSelection) -> Self {
        self.defaults.insert(model.family(), model);
        self
    }

    pub fn default_settings(mut self, model: ModelSelection, thinking: Thinking) -> Self {
        self.thinking_defaults.insert(model.family(), thinking);
        self.defaults.insert(model.family(), model);
        self
    }

    pub fn build(self) -> Harness<D> {
        Harness {
            formats: std::marker::PhantomData,
            inner: Arc::new(Router {
                recipes: self.recipes,
                defaults: self.defaults,
                thinking_defaults: self.thinking_defaults,

                recovery_store: self.recovery_store,
                children: None,
            }),
        }
    }

    pub(crate) fn build_tracked(
        self,
        children: std::sync::Weak<crate::runtime::Ownership>,
    ) -> Harness<D> {
        Harness {
            formats: std::marker::PhantomData,
            inner: Arc::new(Router {
                recipes: self.recipes,
                defaults: self.defaults,
                thinking_defaults: self.thinking_defaults,

                recovery_store: self.recovery_store,
                children: Some(children),
            }),
        }
    }
}

impl<D: CheckpointFormats> Harness<D> {
    pub(crate) fn resolve_options(
        &self,
        options: SpawnOptions,
        parent: ModelSelection,
        thinking: Thinking,
    ) -> nanocode_agent_core::Result<SpawnOptions> {
        self.inner.resolve(options, parent, thinking)
    }

    pub fn builder() -> HarnessBuilder<D> {
        HarnessBuilder::default()
    }

    pub async fn start(&self, model: ModelSelection) -> AgentResult {
        self.start_with(
            SpawnOptions::new()
                .harness(model.family())
                .harness_model(model),
        )
        .await
    }

    pub async fn start_with(&self, options: SpawnOptions) -> AgentResult {
        options.validate_harness()?;
        let family = options
            .selected_family()
            .unwrap_or_else(|| self.inner.default_family());
        let model = options
            .selected_harness_model()
            .unwrap_or_else(|| self.inner.default_model(&family));
        let thinking = self.inner.default_thinking(&model);
        let options = options.resolve(model.clone(), thinking)?;
        self.inner
            .clone()
            .construct(None, options, None, Reopen::default())
            .await
    }

    pub async fn resume(&self, checkpoint: SessionCheckpoint) -> AgentResult {
        checkpoint.validate()?;
        let model = checkpoint.model();
        let options = SpawnOptions::new()
            .harness(model.family())
            .harness_model(model)
            .thinking(checkpoint.thinking())
            .reasoning_mode(checkpoint.reasoning_mode());
        options.validate_harness()?;
        self.inner
            .clone()
            .construct(None, options, None, Reopen::checkpoint(checkpoint))
            .await
    }

    pub async fn open(
        &self,
        store: &crate::durability::SessionStore,
        session_id: &str,
    ) -> AgentResult {
        D::register_formats(store).map_err(catalog_error)?;
        let stored = store.load(session_id).await.map_err(catalog_error)?;
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
        let options = SpawnOptions::new()
            .harness(model.family())
            .harness_model(model)
            .thinking(thinking);
        options.validate_harness()?;
        let durable_state = store
            .prepare_resume(session_id)
            .await
            .map_err(catalog_error)?;
        self.inner
            .clone()
            .construct(
                None,
                options,
                None,
                Reopen {
                    checkpoint: None,
                    durable_state: Some(durable_state),
                },
            )
            .await
    }

    pub fn spawn_factory(&self) -> Arc<dyn AgentFactory> {
        Arc::new(RoutedFactory {
            inner: Arc::clone(&self.inner),
        })
    }
}

#[derive(Default)]
struct Reopen {
    checkpoint: Option<SessionCheckpoint>,
    durable_state: Option<crate::durability::DurableSession>,
}
impl Reopen {
    const fn checkpoint(checkpoint: SessionCheckpoint) -> Self {
        Self {
            checkpoint: Some(checkpoint),
            durable_state: None,
        }
    }
}

struct Router {
    recipes: HashMap<HarnessFamily, Recipe>,
    defaults: HashMap<HarnessFamily, ModelSelection>,
    thinking_defaults: HashMap<HarnessFamily, Thinking>,

    recovery_store: Option<crate::durability::SessionStore>,

    children: Option<std::sync::Weak<crate::runtime::Ownership>>,
}
impl Router {
    fn default_model(&self, family: &HarnessFamily) -> ModelSelection {
        self.defaults
            .get(family)
            .cloned()
            .unwrap_or_else(|| family.default_model())
    }

    fn default_thinking(&self, model: &ModelSelection) -> Thinking {
        if self.defaults.get(&model.family()) == Some(model) {
            self.thinking_defaults
                .get(&model.family())
                .copied()
                .unwrap_or_else(|| model.default_thinking())
        } else {
            model.default_thinking()
        }
    }

    fn resolve(
        &self,
        options: SpawnOptions,
        parent: ModelSelection,
        thinking: Thinking,
    ) -> nanocode_agent_core::Result<SpawnOptions> {
        let family = options.selected_family().unwrap_or_else(|| parent.family());
        let options = if family != parent.family() && options.selected_harness_model().is_none() {
            options.harness_model(self.default_model(&family))
        } else {
            options
        };
        let options = if options.selected_thinking().is_none() {
            match options.selected_harness_model() {
                Some(model) if model != parent => options.thinking(self.default_thinking(&model)),
                _ => options,
            }
        } else {
            options
        };
        options.resolve(parent, thinking)
    }

    fn default_family(&self) -> HarnessFamily {
        if self.recipes.contains_key(&HarnessFamily::Codex) || self.recipes.len() != 1 {
            HarnessFamily::Codex
        } else {
            self.recipes
                .keys()
                .next()
                .expect("one registered family")
                .clone()
        }
    }

    async fn tracked<F>(
        &self,
        construct: F,
    ) -> nanocode_agent_core::Result<Vec<(Agent, AgentEvents)>>
    where
        F: Future<Output = nanocode_agent_core::Result<Vec<(Agent, AgentEvents)>>>,
    {
        if let Some(owner) = self.children.as_ref().and_then(std::sync::Weak::upgrade) {
            return owner.track_children(construct).await;
        }
        construct.await
    }

    async fn tracked_one<F>(&self, construct: F) -> AgentResult
    where
        F: Future<Output = AgentResult>,
    {
        let mut created = self
            .tracked(async { construct.await.map(|child| vec![child]) })
            .await?;
        Ok(created.remove(0))
    }

    async fn construct(
        self: Arc<Self>,
        parent: Option<AgentHandle>,
        options: SpawnOptions,
        host_context: Option<Arc<str>>,
        reopen: Reopen,
    ) -> AgentResult {
        let model = options.selected_harness_model().ok_or_else(|| {
            NanocodeError::InvalidRequest("harness construction requires a resolved model".into())
        })?;
        let thinking = options.selected_thinking().ok_or_else(|| {
            NanocodeError::InvalidRequest("harness construction requires a resolved effort".into())
        })?;
        let recipe = self
            .recipes
            .get(&model.family())
            .ok_or_else(|| {
                NanocodeError::InvalidRequest(format!(
                    "{} harness is not configured for this host",
                    model.family()
                ))
            })?
            .clone();
        let spawn_factory: Arc<dyn AgentFactory> = Arc::new(RoutedFactory { inner: self });
        recipe(HarnessRequest {
            model,
            thinking,
            options,
            parent,
            host_context,
            checkpoint: reopen.checkpoint,
            durable_state: reopen.durable_state,
            spawn_factory,
        })
        .await
    }
}

struct RoutedFactory {
    inner: Arc<Router>,
}
impl AgentFactory for RoutedFactory {
    fn ensure_available(
        &self,
        parent: AgentHandle,
    ) -> BackendFuture<nanocode_agent_core::Result<()>> {
        Box::pin(async move { parent.ensure_available().await })
    }

    fn settings(
        &self,
        parent: AgentHandle,
    ) -> BackendFuture<nanocode_agent_core::Result<(ModelSelection, Thinking)>> {
        Box::pin(async move { parent.settings().await })
    }

    fn fork(&self, parent: AgentHandle, request: ForkRequest) -> BackendFuture<AgentResult> {
        Box::pin(async move { parent.fork(request).await })
    }

    fn spawn(
        &self,
        parent: AgentHandle,
        options: SpawnOptions,
        host_context: Option<Arc<str>>,
    ) -> BackendFuture<AgentResult> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let router = Arc::clone(&inner);
            router
                .tracked_one(async move {
                    parent.ensure_available().await?;
                    options.validate_harness()?;
                    if resolve_family(&parent, &options) == parent.harness_family() {
                        return parent
                            .spawn_native_with_host_context(options, host_context)
                            .await;
                    }

                    let parent_model = parent.harness_model();
                    let options = inner.resolve(
                        options,
                        parent_model.clone(),
                        parent_model.default_thinking(),
                    )?;
                    inner
                        .construct(Some(parent), options, host_context, Reopen::default())
                        .await
                })
                .await
        })
    }

    fn spawn_many(
        &self,
        parent: AgentHandle,
        count: usize,
        observer: Arc<dyn Fn(&str) + Send + Sync>,
        host_context: Option<Arc<str>>,
    ) -> BackendFuture<nanocode_agent_core::Result<Vec<(Agent, AgentEvents)>>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            inner
                .tracked(async move {
                    parent.ensure_available().await?;
                    debug_assert_eq!(
                        resolve_family(&parent, &SpawnOptions::new()),
                        parent.harness_family()
                    );
                    parent
                        .spawn_many_native_with_host_context(
                            count,
                            move |id| observer(id),
                            host_context,
                        )
                        .await
                })
                .await
        })
    }

    fn restore(
        &self,
        parent: AgentHandle,
        checkpoint: SessionCheckpoint,
        host_context: Option<Arc<str>>,
    ) -> BackendFuture<AgentResult> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let router = Arc::clone(&inner);
            router
                .tracked_one(async move {
                    parent.ensure_available().await?;
                    checkpoint.validate()?;

                    let checkpoint = match &inner.recovery_store {
                        Some(store) => store
                            .recovery_checkpoint(checkpoint)
                            .await
                            .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?,
                        None => checkpoint,
                    };
                    if checkpoint.family() == parent.harness_family() {
                        return parent
                            .restore_native_runtime(checkpoint, host_context)
                            .await;
                    }
                    let model = checkpoint.model();
                    let options = SpawnOptions::new()
                        .harness(model.family())
                        .harness_model(model)
                        .thinking(checkpoint.thinking())
                        .reasoning_mode(checkpoint.reasoning_mode());
                    options.validate_harness()?;
                    inner
                        .construct(
                            Some(parent),
                            options,
                            host_context,
                            Reopen::checkpoint(checkpoint),
                        )
                        .await
                })
                .await
        })
    }
}

fn resolve_family(parent: &AgentHandle, options: &SpawnOptions) -> HarnessFamily {
    options.selected_family().unwrap_or(parent.harness_family())
}

pub trait CheckpointFormats: Clone + Default {
    fn register_formats(store: &crate::durability::SessionStore) -> crate::durability::Result<()>;
}

#[derive(Clone, Default)]
pub struct NoFormats;
impl CheckpointFormats for NoFormats {
    fn register_formats(_store: &crate::durability::SessionStore) -> crate::durability::Result<()> {
        Ok(())
    }
}

pub fn session_store<D: CheckpointFormats, S: crate::durability::StateStore + 'static>(
    store: S,
) -> crate::durability::Result<crate::durability::SessionStore> {
    let store = crate::durability::SessionStore::new(store)?;
    D::register_formats(&store)?;
    Ok(store)
}
