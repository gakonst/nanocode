use nanocode_agent_core::SpawnOptions;

pub struct SpawnRoute {
    pub options: SpawnOptions,
    pub reference: String,
}

impl SpawnRoute {
    pub(crate) fn validate(&self, requested: SpawnOptions) -> std::io::Result<()> {
        let model = self
            .options
            .selected_harness_model()
            .ok_or_else(|| std::io::Error::other("subagent route must select a model"))?;
        let thinking = self
            .options
            .selected_thinking()
            .ok_or_else(|| std::io::Error::other("subagent route must select thinking"))?;
        self.options
            .validate_harness()
            .map_err(std::io::Error::other)?;
        if requested
            .selected_harness()
            .is_some_and(|family| family != model.family())
        {
            return Err(std::io::Error::other(
                "subagent route conflicts with explicit harness",
            ));
        }
        if let (Some(requested), Some(selected)) = (
            requested.selected_reasoning_mode(),
            self.options.selected_reasoning_mode(),
        ) && requested != selected
        {
            return Err(std::io::Error::other(
                "subagent route conflicts with explicit reasoning mode",
            ));
        }
        if requested
            .selected_harness_model()
            .is_some_and(|value| value != model)
            || requested
                .selected_thinking()
                .is_some_and(|value| value != thinking)
        {
            return Err(std::io::Error::other(
                "subagent route conflicts with explicit override",
            ));
        }
        if !model.supports_thinking(thinking) {
            return Err(std::io::Error::other(
                "subagent route selected unsupported thinking",
            ));
        }
        if self.reference.trim().is_empty() {
            return Err(std::io::Error::other("empty subagent route reference"));
        }
        Ok(())
    }
}

pub enum SpawnDecision {
    Routed(SpawnRoute),
    Native { reference: String },
}

impl SpawnDecision {
    pub(crate) fn validate(&self, requested: SpawnOptions) -> std::io::Result<()> {
        match self {
            Self::Routed(route) => route.validate(requested),
            Self::Native { reference } => {
                if reference.trim().is_empty() {
                    return Err(std::io::Error::other("invalid native subagent choice"));
                }
                Ok(())
            }
        }
    }

    pub(crate) fn options(&self, requested: SpawnOptions) -> SpawnOptions {
        match self {
            Self::Routed(route) => {
                let mut options = route.options.clone();
                if options.selected_reasoning_mode().is_none() {
                    options.reasoning_mode = requested.selected_reasoning_mode();
                }
                options
            }
            Self::Native { .. } => requested,
        }
    }

    pub(crate) fn reference(&self) -> &str {
        match self {
            Self::Routed(route) => &route.reference,
            Self::Native { reference } => reference,
        }
    }
}

#[cfg(not(target_family = "wasm"))]
type RouteFuture<'a, T> = futures_util::future::BoxFuture<'a, T>;

#[cfg(target_family = "wasm")]
type RouteFuture<'a, T> = futures_util::future::LocalBoxFuture<'a, T>;

pub trait SpawnRouter: Send + Sync {
    fn resolve<'life0, 'life1, 'life2, 'life3, 'life4, 'async_trait>(
        &'life0 self,
        parent_session_id: &'life1 str,
        role: &'life2 str,
        task: &'life3 str,
        options: SpawnOptions,
        host_context: Option<&'life4 str>,
    ) -> RouteFuture<'async_trait, std::io::Result<SpawnRoute>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        'life4: 'async_trait,
        Self: 'async_trait;

    fn resolve_spawn<'life0, 'life1, 'life2, 'life3, 'life4, 'async_trait>(
        &'life0 self,
        parent_session_id: &'life1 str,
        role: &'life2 str,
        task: &'life3 str,
        options: SpawnOptions,
        host_context: Option<&'life4 str>,
    ) -> RouteFuture<'async_trait, std::io::Result<SpawnDecision>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        'life4: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: std::io::Result<SpawnDecision> = {
                self.resolve(parent_session_id, role, task, options, host_context)
                    .await
                    .map(SpawnDecision::Routed)
            };
            output
        })
    }

    fn bind(
        &self,
        parent_session_id: &str,
        child_session_id: &str,
        reference: &str,
        host_context: Option<&str>,
    ) -> std::io::Result<()>;
}
