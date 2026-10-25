use super::spawn::{validate_model_reasoning_mode, validate_model_thinking};
use super::*;

#[derive(Clone)]
pub struct CodexBuilder<F = StandardServiceFactory> {
    pub(crate) config: ModelConfig,
    pub(super) tools: ToolsConfiguration,
    pub(super) workspace: Option<PathBuf>,
    pub(super) session_id: Option<SessionId>,
    pub(super) prompt_cache: PromptCacheConfig,
    pub(super) codex: CodexCompatibility,
    pub(super) resume: Option<SessionSnapshot>,
    pub(super) lineage: Option<Lineage>,

    pub(super) service_tier_explicit: bool,
    pub(super) reasoning_mode_explicit: bool,

    pub(super) session_id_explicit: bool,
    pub(super) factory: F,
}

impl<F> BuilderBackend for OpenAi<F>
where
    F: ResponsesServiceFactory,
{
    type Builder = CodexBuilder<F>;

    fn into_builder(self) -> Self::Builder {
        let (config, factory) = into_openai_parts(self);
        CodexBuilder {
            config,
            tools: ToolsConfiguration::Shared(Tools::default()),
            workspace: None,
            session_id: None,
            prompt_cache: PromptCacheConfig::default(),
            codex: CodexCompatibility::default(),
            resume: None,
            lineage: None,
            service_tier_explicit: false,
            reasoning_mode_explicit: false,
            session_id_explicit: false,
            factory,
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct PromptCacheConfig {
    pub(super) key: Option<String>,
    pub(super) per_session: bool,
    pub(super) shared: Option<SharedPromptCache>,
}

#[derive(Clone)]
pub(super) struct CodexCompatibility {
    pub(super) instant_tool_steering: bool,
    pub(super) context: ContextSourceConfig,
    pub(super) execution: ExecutionConfig,
    pub(super) before_compaction: Option<Arc<dyn execution::BeforeCompaction>>,
    pub(super) images: ImageHistory,
    pub(super) spawn_factory: Option<Arc<dyn backend::AgentFactory>>,
    pub(super) host_context: Option<Arc<str>>,
    pub(super) child_journal: Option<backend::ChildJournal>,
    pub(super) initial_developer_messages: Vec<String>,
}

impl Default for CodexCompatibility {
    fn default() -> Self {
        Self {
            instant_tool_steering: true,
            context: ContextSourceConfig::default(),
            execution: ExecutionConfig::default(),
            before_compaction: None,
            images: ImageHistory::default(),
            spawn_factory: None,
            host_context: None,
            child_journal: None,
            initial_developer_messages: Vec::new(),
        }
    }
}

impl<F> CodexBuilder<F> {
    #[must_use]
    pub const fn stateless_http(mut self) -> Self {
        self.config.responses_transport = ResponsesTransport::Https;
        self.config.responses_history = ResponsesHistory::FullReplay;
        self.config.store_responses = false;
        self.config.websocket_warmup = false;
        self
    }

    #[must_use]
    pub const fn instant_tool_steering(mut self, enabled: bool) -> Self {
        self.codex.instant_tool_steering = enabled;
        self
    }

    #[must_use]
    pub fn before_compaction(mut self, hook: impl execution::BeforeCompaction + 'static) -> Self {
        self.codex.before_compaction = Some(Arc::new(hook));
        self
    }

    #[must_use]
    pub fn images(mut self, images: ImageHistory) -> Self {
        self.codex.images = images;
        self
    }

    pub fn resume(mut self, checkpoint: SessionCheckpoint) -> Result<Self> {
        let snapshot = ChildState::from_checkpoint(checkpoint)?;
        self = self
            .model(snapshot.model)
            .thinking(snapshot.thinking)
            .reasoning_mode(snapshot.reasoning_mode)
            .service_tier(snapshot.service_tier);
        self.session_id = Some(snapshot.session_id.parse().map_err(|error| {
            NanocodeError::InvalidCheckpoint(format!("invalid child session: {error}"))
        })?);

        self.session_id_explicit = false;
        self.resume = snapshot.conversation;
        self.lineage = Some(snapshot.lineage);
        if snapshot.stateless_http {
            self.config.responses_transport = ResponsesTransport::Https;
            self.config.responses_history = ResponsesHistory::FullReplay;
            self.config.store_responses = false;
            self.config.websocket_warmup = false;
        }
        Ok(self)
    }

    #[must_use]
    pub fn host_context(mut self, context: Option<Arc<str>>) -> Self {
        self.codex.host_context = context;
        self
    }

    #[must_use]
    pub fn child_journal(mut self, journal: backend::ChildJournal) -> Self {
        self.codex.child_journal = Some(journal);
        self
    }

    #[must_use]
    pub fn spawn_factory(mut self, factory: Arc<dyn backend::AgentFactory>) -> Self {
        self.codex.spawn_factory = Some(factory);
        self
    }

    #[must_use]
    pub fn model(mut self, model: Model) -> Self {
        self.config.retarget(model);
        if !self.config.thinking_explicit {
            self.config.thinking = model.default_thinking();
        }
        if self.config.context_window_tokens > model.max_context_window_tokens() {
            self.config.context_window_tokens = model.max_context_window_tokens();
        }
        self
    }

    #[must_use]
    pub fn instructions(mut self, instructions: impl Into<Arc<str>>) -> Self {
        self.config.system_prompt = Some(instructions.into());
        self
    }

    #[must_use]
    pub fn model_instructions(mut self, model: Model, instructions: impl Into<Arc<str>>) -> Self {
        let configured = &mut self.config.model_system_prompts;
        configured.retain(|(existing, _)| *existing != model);
        configured.push((model, instructions.into()));
        self
    }

    #[must_use]
    pub fn additional_instructions(mut self, instructions: impl Into<Arc<str>>) -> Self {
        self.config.additional_instructions = Some(instructions.into());
        self
    }

    #[must_use]
    pub const fn thinking(mut self, thinking: Thinking) -> Self {
        self.config.thinking = thinking;
        self.config.thinking_explicit = true;
        self
    }

    #[must_use]
    pub const fn fast_mode(self, enabled: bool) -> Self {
        self.service_tier(ServiceTier::from_fast_mode(enabled))
    }

    #[must_use]
    pub const fn service_tier(mut self, service_tier: ServiceTier) -> Self {
        self.config.service_tier = service_tier;
        self.service_tier_explicit = true;
        self
    }

    #[must_use]
    pub const fn context_window_tokens(mut self, tokens: u64) -> Self {
        let maximum = self.config.model.max_context_window_tokens();
        self.config.context_window_tokens = if tokens > maximum { maximum } else { tokens };
        self
    }

    #[must_use]
    pub const fn reasoning_mode(mut self, reasoning_mode: ReasoningMode) -> Self {
        self.config.reasoning_mode = reasoning_mode;
        self.reasoning_mode_explicit = true;
        self
    }

    #[must_use]
    pub fn tools(mut self, tools: Tools) -> Self {
        self.tools = ToolsConfiguration::Shared(tools);
        self
    }

    #[must_use]
    pub fn tools_factory<T>(mut self, factory: T) -> Self
    where
        T: Fn(AgentHandle) -> std::result::Result<Tools, ToolsBuildError> + Send + Sync + 'static,
    {
        self.tools = ToolsConfiguration::PerAgent(Arc::new(factory));
        self
    }

    pub(crate) fn runtime_tools(mut self, runtime: nanocode_agent_core::RuntimeTools) -> Self {
        if runtime.is_empty() {
            return self;
        }
        let configured = self.tools.clone();
        self.tools = ToolsConfiguration::PerAgent(Arc::new(move |handle| {
            let tools = match &configured {
                ToolsConfiguration::Shared(tools) => tools.clone(),
                ToolsConfiguration::PerAgent(factory) => factory(handle.clone())?,
            };
            runtime.install(tools, &handle)
        }));
        self
    }

    #[must_use]
    pub fn workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    #[must_use]
    pub fn execution_environment(mut self, environment: ExecutionEnvironment) -> Self {
        self.codex.context.set_execution_environment(environment);
        self
    }

    #[must_use]
    pub fn session_id(mut self, session_id: SessionId) -> Self {
        if self.session_id.is_some_and(|current| current != session_id) {
            self.lineage = None;
        }
        self.session_id = Some(session_id);
        self.session_id_explicit = true;
        self
    }

    #[must_use]
    pub fn prompt_cache_key(mut self, prompt_cache_key: impl Into<String>) -> Self {
        self.prompt_cache.key = Some(prompt_cache_key.into());
        self
    }

    #[must_use]
    pub const fn per_session_prompt_cache_key(mut self) -> Self {
        self.prompt_cache.per_session = true;
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn default_prompt_cache_key(mut self, prompt_cache_key: impl Into<String>) -> Self {
        if self.prompt_cache.per_session && self.prompt_cache.key.is_none() {
            self.prompt_cache.key = Some(prompt_cache_key.into());
        }
        self
    }

    #[must_use]
    pub fn shared_prompt_cache(mut self) -> Self {
        self.prompt_cache.shared = Some(SharedPromptCache::default());
        self
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn nanocode_home(mut self, nanocode_home: impl Into<PathBuf>) -> Self {
        self.codex.context.set_nanocode_home(nanocode_home.into());
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn resume_native_snapshot(mut self, snapshot: SessionSnapshot) -> Self {
        if let Some(thinking) = snapshot.thinking()
            && !self.config.thinking_explicit
        {
            self.config.thinking = thinking;
        }
        if let Some(service_tier) = snapshot.service_tier()
            && !self.service_tier_explicit
        {
            self.config.service_tier = service_tier;
        }
        if let Some(reasoning_mode) = snapshot.reasoning_mode()
            && !self.reasoning_mode_explicit
        {
            self.config.reasoning_mode = reasoning_mode;
        }
        self.resume = Some(snapshot);
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn initial_settings(
        mut self,
        model: Model,
        thinking: Thinking,
        service_tier: ServiceTier,
    ) -> Self {
        let explicit_thinking = self.config.thinking_explicit;
        self = self.model(model);
        if !explicit_thinking {
            self.config.thinking = thinking;
        }
        if !self.service_tier_explicit {
            self.config.service_tier = service_tier;
        }
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn lineage(mut self, lineage: Lineage) -> Self {
        self.lineage = Some(lineage);
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn explicit_session_id(&self) -> Option<SessionId> {
        if self.session_id_explicit {
            self.session_id
        } else {
            None
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn resume_snapshot(&self) -> Option<&SessionSnapshot> {
        self.resume.as_ref()
    }

    pub(crate) fn initial_developer_messages(mut self, messages: Vec<String>) -> Self {
        self.codex.initial_developer_messages = messages;
        self
    }

    #[must_use]
    pub fn execution_policy(mut self, policy: Arc<dyn execution::ExecutionPolicy>) -> Self {
        self.codex.execution.set_policy(policy);
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub fn execution_policy_factory<P>(mut self, factory: P) -> Self
    where
        P: Fn() -> Result<Arc<dyn execution::ExecutionPolicy>> + Send + Sync + 'static,
    {
        self.codex.execution.set_policy_factory(Arc::new(factory));
        self
    }
}

#[cfg(not(target_family = "wasm"))]
impl<F> CodexBuilder<F>
where
    F: ResponsesServiceFactory + Send + Sync + 'static,
    F::Service: Service<ResponsesAttempt, Response = ResponsesServiceResponse> + Send + 'static,
    <F::Service as Service<ResponsesAttempt>>::Error: Into<ResponseError> + Send + 'static,
    <F::Service as Service<ResponsesAttempt>>::Future: Send,
{
    pub fn build(self) -> Result<(Agent, AgentEvents)> {
        build(self)
    }
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
impl<F> CodexBuilder<F>
where
    F: ResponsesServiceFactory + 'static,
    F::Service: Service<ResponsesAttempt, Response = ResponsesServiceResponse> + 'static,
    <F::Service as Service<ResponsesAttempt>>::Error: Into<ResponseError> + 'static,
{
    pub fn build(self) -> Result<(Agent, AgentEvents)> {
        build(self)
    }
}

fn build<F>(builder: CodexBuilder<F>) -> Result<(Agent, AgentEvents)>
where
    F: ResponsesServiceFactory + AgentFactory + 'static,
    F::Service:
        Service<ResponsesAttempt, Response = ResponsesServiceResponse> + AgentSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Error: Into<ResponseError> + AgentSend + 'static,
    <F::Service as Service<ResponsesAttempt>>::Future: AgentSend,
{
    if builder.resume.is_none() {
        validate_model_thinking(builder.config.model, builder.config.thinking)?;
        validate_model_reasoning_mode(builder.config.model, builder.config.reasoning_mode)?;

        if builder.service_tier_explicit {
            crate::ModelSelection::Codex(builder.config.model)
                .capabilities(crate::ModelTransport::Native)
                .check_service_tier(builder.config.service_tier)?;
        }
    }
    validate(&builder.config, builder.prompt_cache.key.as_deref())?;
    validate_execution_environment(builder.codex.context.execution_environment())?;
    let config = Arc::new(builder.config);
    let factory = builder.factory;
    let service_factory: ServiceFactory =
        Arc::new(move |config| Box::new(ResponsesClient::new(factory.make(config))));
    build_agent(
        config,
        builder.tools,
        builder.workspace,
        builder.session_id,
        builder.prompt_cache,
        builder.codex,
        builder.resume,
        builder.lineage,
        service_factory,
    )
}

fn validate_execution_environment(environment: Option<&ExecutionEnvironment>) -> Result<()> {
    let Some(environment) = environment else {
        return Ok(());
    };
    let current_date = environment.current_date();
    if current_date.trim().is_empty() {
        return Err(NanocodeError::InvalidRequest(
            "execution-environment current date must not be empty".to_owned(),
        ));
    }
    if !is_iso_date(current_date.trim()) {
        return Err(NanocodeError::InvalidRequest(
            "execution-environment current date must use YYYY-MM-DD".to_owned(),
        ));
    }
    if environment.timezone.trim().is_empty() {
        return Err(NanocodeError::InvalidRequest(
            "execution-environment timezone must not be empty".to_owned(),
        ));
    }
    if environment
        .project_instructions
        .as_deref()
        .is_some_and(|instructions| instructions.trim().is_empty())
    {
        return Err(NanocodeError::InvalidRequest(
            "execution-environment project instructions must not be empty".to_owned(),
        ));
    }
    if environment
        .working_directory
        .as_deref()
        .is_some_and(|directory| directory.trim().is_empty())
    {
        return Err(NanocodeError::InvalidRequest(
            "execution-environment working directory must not be empty".to_owned(),
        ));
    }
    if environment
        .shell
        .as_deref()
        .is_some_and(|shell| shell.trim().is_empty())
    {
        return Err(NanocodeError::InvalidRequest(
            "execution-environment shell must not be empty".to_owned(),
        ));
    }
    Ok(())
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let Some(year) = decimal(&bytes[..4]) else {
        return false;
    };
    let Some(month) = decimal(&bytes[5..7]) else {
        return false;
    };
    let Some(day) = decimal(&bytes[8..]) else {
        return false;
    };
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

fn decimal(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |value, byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u32::from(*byte - b'0'))
    })
}
