use crate::__private::model_contract as model_tool_contract;
mod continuation;
mod images;
mod lifecycle;
mod responses;
mod side;
mod state;
mod tool_calls;
mod turn;

use continuation::ExecutionPhase;
use images::*;
use lifecycle::*;
use responses::*;
use state::*;
use tool_calls::*;

use std::{
    any::Any,
    collections::{HashMap, VecDeque},
    panic::AssertUnwindSafe,
    sync::Arc,
};

use crate::client::{
    __private::{
        EventSink, ManagedSessionState, ModelConfig, RequestHistory, ResponsesAttemptFactory,
        TransportFallback, assign_missing_response_item_id, compaction,
        responses_lite_request_prefix, with_code_mode_tool_names,
    },
    Model, Prompt, Thinking,
    events::AgentEventKind,
    pricing::{ServiceTier, estimate_for_model},
    responses::{
        ContentItem, FunctionOutputBody, MessageRole, RequestProfile, ResponseHistory,
        ResponseItem, ToolDefinition, Usage,
    },
    tower::{CodeCall, CodeCallKind, GenerationOutput as TurnResult, ResponsesOutput},
    transport::{ResponsesError, ResponsesTransport, TransportStats, TransportStatsSnapshot},
};
use futures_util::{FutureExt, StreamExt, stream::FuturesOrdered};
use nanocode_platform::{Duration, Instant, UNIX_EPOCH};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use tokio::sync::watch;
use tracing::{Instrument, info, info_span};

use super::{
    CompactionCompleted, CompactionFailed, CompactionStarted, ModelCallCompleted, ModelCallFailed,
    ModelCallStarted, ModelCallTiming, RunError, RunStarted, RunStats, RunSteered,
    ToolCallArguments, ToolCallEvent, ToolNotificationEvent, ToolResultEvent, WarmupCompleted,
    WarmupFailed, WarmupStarted,
    context::{ContextBaseline, ContextSnapshot, ContextState},
    display_endpoint, elapsed_ns,
    input::{
        custom_tool_notification, custom_tool_output, developer_context, function_tool_output,
        host_context, prompt_messages, task_input, tool_search_output, turn_aborted,
    },
    terminal_payload,
};
use crate::{
    NanocodeError, Result,
    agent::{ContextSource, ExecutionSteps, ResponsesClientBox, execution::QueuedSteer},
    prompt_cache::ModelPromptCache,
    usage::TurnUsage,
};
use nanocode_code_mode::contract::{HistorySource, SharedHistory};
use nanocode_tool_host::{
    ToolContext, Tools,
    code_mode::{CodeModeExecution, CodeModeObserver, CodeModeUpdate},
    contract::{DEFAULT_TOOL_OUTPUT_TOKENS, ToolInput, ToolOutput, ToolOutputBody},
    image::{ImageHistory, prepare_output_images},
    runtime::{OwnedToolContext, ToolRuntime, ToolRuntimeControl},
};

pub(crate) struct ModelRun {
    events: EventSink,
    provider_session_id: Arc<str>,
    config: Arc<ModelConfig>,
    model: Model,
    thinking: Thinking,
    service_tier: ServiceTier,
    client: ResponsesClientBox,
    transport_stats: Arc<TransportStats>,
    transport_fallback: TransportFallback,
    started_at: Instant,
    operation_started_at: Option<Duration>,
    elapsed_before_attempt: Duration,
    stats: RunStats,
    transport_baseline: TransportStatsSnapshot,
    session: Option<ModelSessionState>,
    active_tools: Option<ToolRuntimeControl>,
    active_tool_calls: Vec<ActiveToolCall>,
    terminated_cell_output: Vec<(String, String)>,
    active_tool_batch_started_at: Option<Instant>,
    tool_call_indices: HashMap<Box<str>, u32>,
    tools: Tools,
    prompt_cache: ModelPromptCache,
    context_source: ContextSource,
    host_context: Option<Arc<str>>,

    instruction_revision: Option<u64>,
    global_instructions: Option<Arc<str>>,
    force_compaction: bool,
    pending_developer_messages: Vec<ResponseItem>,
    execution_steps: Option<ExecutionSteps>,
    before_compaction: Option<Arc<dyn crate::execution::BeforeCompaction>>,
    images: ImageHistory,
}

pub(crate) struct TurnSteering {
    pub(crate) preempt: watch::Receiver<u64>,
    pub(crate) instant_tool_steering: bool,
    pub(crate) receiver: crate::agent::execution::SteerQueue,
    pub(crate) retained: Vec<QueuedSteer>,
    pub(crate) model_call_index: Arc<std::sync::atomic::AtomicU32>,
}

pub(crate) enum ModelTurnOutcome {
    Completed(CompletedModelTurn),
    Cancelled(ModelCheckpoint),
    Failed {
        error: NanocodeError,
        checkpoint: ModelCheckpoint,
    },
}

pub(crate) enum ModelCompactOutcome {
    Completed(ModelCheckpoint),
    Cancelled(ModelCheckpoint),
    Failed {
        error: NanocodeError,
        checkpoint: ModelCheckpoint,
    },
}

pub(crate) struct CompletedModelTurn {
    pub(crate) final_message: String,
    pub(crate) usage: TurnUsage,
    pub(crate) checkpoint: ModelCheckpoint,
}

#[derive(Clone)]
pub(crate) struct ModelCheckpoint {
    workspace: String,
    provider_session_id: Arc<str>,
    conversation: ConversationState,
    request_prefix: Arc<[ResponseItem]>,
    prompt_cache_key: Arc<str>,
    preserve_inherited_delta: bool,
    global_instructions: Option<Arc<str>>,
    context_baseline: ContextBaseline,
}

pub(crate) struct PreparedCheckpoint {
    pub(crate) checkpoint: ModelCheckpoint,
    pub(crate) runtime: ToolRuntime,
    pub(crate) context_source: ContextSource,
    selected_agents_md: Option<Arc<str>>,
}

pub(crate) struct HistoryCheckpoint {
    pub(crate) workspace: String,
    pub(crate) provider_session_id: Arc<str>,
    pub(crate) canonical_context: ResponseItem,
    pub(crate) history: Vec<ResponseItem>,
    pub(crate) client_authored: std::collections::BTreeSet<String>,
    pub(crate) prompt_cache_key: Arc<str>,
    pub(crate) context_baseline: Option<ContextBaseline>,
    pub(crate) reasoning: crate::reasoning::ReasoningState,
}

impl ModelCheckpoint {
    pub(crate) fn workspace(&self) -> &str {
        &self.workspace
    }
    pub(crate) fn history(&self) -> crate::client::responses::ResponseHistory {
        self.conversation.shared_history()
    }

    pub(crate) fn request_prefix(&self) -> &[ResponseItem] {
        &self.request_prefix
    }

    pub(crate) fn prompt_cache_key(&self) -> &str {
        &self.prompt_cache_key
    }

    pub(crate) fn canonical_context(&self) -> &ResponseItem {
        &self.conversation.canonical_context
    }

    pub(crate) const fn client_authored(&self) -> &std::collections::BTreeSet<String> {
        self.conversation.managed.client_authored()
    }

    pub(crate) const fn reasoning(&self) -> &crate::reasoning::ReasoningState {
        &self.conversation.reasoning
    }

    pub(crate) fn restore_reasoning(&mut self, reasoning: crate::reasoning::ReasoningState) {
        self.conversation.reasoning = reasoning;
    }

    pub(crate) fn context_usage(&self) -> crate::session::ContextUsage {
        let (usage, server_reasoning_included) = self.conversation.managed.context_usage();
        crate::session::ContextUsage {
            usage: usage.cloned(),
            server_reasoning_included,
            is_estimate: self.conversation.managed.context_usage_is_estimate(),
        }
    }

    pub(crate) fn restore_context_usage(&mut self, usage: &crate::session::ContextUsage) {
        self.conversation.managed.restore_context_usage(
            usage.usage.as_ref(),
            usage.server_reasoning_included,
            usage.is_estimate,
        );
    }

    pub(crate) fn snapshot_history(&self) -> Vec<ResponseItem> {
        self.conversation.flattened_history()
    }

    pub(crate) const fn context_baseline(&self) -> &ContextBaseline {
        &self.context_baseline
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "checkpoint restoration keeps each retained boundary explicit"
    )]
    pub(crate) fn resume(
        workspace: String,
        provider_session_id: Arc<str>,
        request_prefix: Vec<ResponseItem>,
        prompt_cache_key: Arc<str>,
        canonical_context: ResponseItem,
        history: Vec<ResponseItem>,
        client_authored: std::collections::BTreeSet<String>,
        global_instructions: Option<Arc<str>>,
        context_baseline: Option<ContextBaseline>,
    ) -> Result<Self> {
        let context_baseline =
            context_baseline.unwrap_or_else(|| ContextBaseline::reconstruct(&history));
        let mut conversation = ConversationState::resume(canonical_context, history)?;
        conversation
            .managed
            .restore_client_authored(client_authored);
        Ok(Self {
            workspace,
            provider_session_id,
            conversation,
            request_prefix: Arc::from(request_prefix),
            prompt_cache_key,
            preserve_inherited_delta: false,
            global_instructions,
            context_baseline,
        })
    }
}

impl ModelRun {
    #[allow(
        clippy::too_many_arguments,
        reason = "the private run owns each injected lifecycle component directly"
    )]
    pub(crate) fn new(
        events: EventSink,
        provider_session_id: Arc<str>,
        config: Arc<ModelConfig>,
        client: ResponsesClientBox,
        transport_stats: Arc<TransportStats>,
        tools: Tools,
        prompt_cache: ModelPromptCache,
        context_source: ContextSource,
        host_context: Option<Arc<str>>,
    ) -> Self {
        let model = config.model;
        let thinking = config.thinking;
        let service_tier = config.service_tier;
        let global_instructions = context_source.global_instructions();
        Self {
            events,
            provider_session_id,
            config,
            model,
            thinking,
            service_tier,
            client,
            transport_stats,
            transport_fallback: TransportFallback::default(),
            started_at: Instant::now(),
            operation_started_at: None,
            elapsed_before_attempt: Duration::ZERO,
            stats: RunStats::default(),
            transport_baseline: TransportStatsSnapshot::default(),
            session: None,
            active_tools: None,
            active_tool_calls: Vec::new(),
            terminated_cell_output: Vec::new(),
            active_tool_batch_started_at: None,
            tool_call_indices: HashMap::new(),
            tools,
            prompt_cache,
            context_source,
            host_context,
            instruction_revision: None,
            global_instructions,
            force_compaction: false,
            pending_developer_messages: Vec::new(),
            execution_steps: None,
            before_compaction: None,
            images: ImageHistory::default(),
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the private run owns each injected lifecycle component directly"
    )]
    pub(crate) fn from_checkpoint(
        events: EventSink,
        config: Arc<ModelConfig>,
        client: ResponsesClientBox,
        transport_stats: Arc<TransportStats>,
        tools: Tools,
        prompt_cache: ModelPromptCache,
        prepared: PreparedCheckpoint,
        host_context: Option<Arc<str>>,
    ) -> Self {
        let PreparedCheckpoint {
            checkpoint,
            runtime,
            context_source,
            selected_agents_md,
        } = prepared;
        let provider_session_id = Arc::clone(&checkpoint.provider_session_id);
        let active_tools = runtime.control();
        let (_, code_mode_tool_names) = model_tool_contract(&runtime, events.request_id());
        let transport_fallback = TransportFallback::default();
        let factory = ResponsesAttemptFactory::new(
            with_code_mode_tool_names(
                RequestProfile::new(
                    checkpoint.provider_session_id.to_string(),
                    checkpoint.prompt_cache_key.to_string(),
                    Arc::clone(&checkpoint.request_prefix),
                )
                .with_thread_id(events.request_id()),
                code_mode_tool_names,
            ),
            events.clone(),
            Arc::clone(&transport_stats),
        )
        .with_transport_fallback(&transport_fallback);
        let model = config.model;
        let thinking = config.thinking;
        let service_tier = config.service_tier;
        let context_source =
            context_source.with_fallback_global(checkpoint.global_instructions.clone());
        let global_instructions = context_source.global_instructions();
        let mut context = ContextState::new(selected_agents_md, checkpoint.context_baseline);
        context.restore_host_instructions(
            checkpoint.conversation.managed.history(),
            &config.host_instructions(),
        );
        Self {
            events,
            provider_session_id,
            config,
            model,
            thinking,
            service_tier,
            client,
            transport_stats,
            transport_fallback,
            started_at: Instant::now(),
            operation_started_at: None,
            elapsed_before_attempt: Duration::ZERO,
            stats: RunStats::default(),
            transport_baseline: TransportStatsSnapshot::default(),
            session: Some(ModelSessionState {
                workspace: checkpoint.workspace,
                tools: runtime,
                factory,
                conversation: checkpoint.conversation,
                context,
                preserve_inherited_delta: checkpoint.preserve_inherited_delta,
            }),
            active_tools: Some(active_tools),
            active_tool_calls: Vec::new(),
            terminated_cell_output: Vec::new(),
            active_tool_batch_started_at: None,
            tool_call_indices: HashMap::new(),
            tools,
            prompt_cache,
            context_source,
            host_context,
            instruction_revision: None,
            global_instructions,
            force_compaction: false,
            pending_developer_messages: Vec::new(),
            execution_steps: None,
            before_compaction: None,
            images: ImageHistory::default(),
        }
    }

    pub(crate) fn set_before_compaction(
        &mut self,
        hook: Option<Arc<dyn crate::execution::BeforeCompaction>>,
    ) {
        self.before_compaction = hook;
    }

    pub(crate) fn set_images(&mut self, images: ImageHistory) {
        self.images = images;
    }

    pub(crate) fn set_host_context(&mut self, host_context: Option<Arc<str>>) {
        self.host_context = host_context;
    }

    pub(crate) fn set_events(&mut self, events: EventSink) {
        if let Some(session) = &mut self.session {
            session.factory.set_events(events.clone());
        }
        self.events = events;
    }

    pub(crate) fn replace_client(&mut self, client: ResponsesClientBox) {
        self.client = client;
    }

    pub(crate) fn preconnect(
        &mut self,
    ) -> Option<crate::agent::execution::ExecutionFuture<'static, ()>> {
        if !matches!(
            self.config.responses_transport,
            ResponsesTransport::WebSocket
        ) {
            return None;
        }
        let factory = ResponsesAttemptFactory::new(
            RequestProfile::new(
                self.provider_session_id.to_string(),
                String::new(),
                Arc::from([]),
            )
            .with_thread_id(self.events.request_id()),
            self.events.clone(),
            Arc::clone(&self.transport_stats),
        )
        .with_transport_fallback(&self.transport_fallback);
        self.client
            .preconnect(factory.connect(self.model, self.thinking, self.service_tier))
    }

    pub(crate) async fn shutdown(&mut self) {
        if let Some(tools) = &self.active_tools {
            tools.cancel().await;
        }
    }

    pub(crate) fn developer_notice_current(&self, prefix: &str, text: &str) -> bool {
        self.session
            .as_ref()
            .and_then(|session| {
                session
                    .conversation
                    .managed
                    .history()
                    .filter_map(|item| match item {
                        ResponseItem::Message {
                            role: MessageRole::Developer,
                            content,
                            ..
                        } => match content.as_slice() {
                            [ContentItem::InputText { text }] if text.starts_with(prefix) => {
                                Some(text)
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                    .last()
            })
            .is_some_and(|latest| latest.ends_with(text))
    }

    pub(crate) fn append_developer_message(
        &mut self,
        text: String,
        requested_workspace: Option<&str>,
    ) -> Result<ModelCheckpoint> {
        let item = ResponseItem::message(
            MessageRole::Developer,
            [ContentItem::InputText {
                text: text.into_boxed_str(),
            }],
        );
        if self.session.is_none() {
            self.session = Some(self.empty_session(requested_workspace)?);
        }
        let session = self.session.as_mut().ok_or_else(|| {
            NanocodeError::InvalidCheckpoint(
                "developer context did not establish a model session".to_owned(),
            )
        })?;
        if !self.pending_developer_messages.is_empty() {
            session
                .conversation
                .append_client(self.pending_developer_messages.drain(..));
        }
        session.conversation.append_client([item]);
        session.conversation.commit_tail();
        session.preserve_inherited_delta = true;
        Ok(ModelCheckpoint {
            workspace: session.workspace.clone(),
            provider_session_id: Arc::from(session.factory.profile().session_id()),
            conversation: session.conversation.clone(),
            request_prefix: session.factory.profile().shared_prefix(),
            prompt_cache_key: Arc::from(session.factory.profile().prompt_cache_key()),
            preserve_inherited_delta: true,
            global_instructions: self.global_instructions.clone(),
            context_baseline: session.context.baseline(),
        })
    }

    fn empty_session(&mut self, requested_workspace: Option<&str>) -> Result<ModelSessionState> {
        let session = self.fresh_session(requested_workspace)?;
        self.active_tools = Some(session.tools.control());
        Ok(session)
    }

    pub(crate) fn side_checkpoint(
        &self,
        requested_workspace: Option<&str>,
    ) -> Result<ModelCheckpoint> {
        let fresh;
        let session = match &self.session {
            Some(session) => session,
            None => {
                fresh = self.fresh_session(requested_workspace)?;
                &fresh
            }
        };
        Ok(ModelCheckpoint {
            workspace: session.workspace.clone(),
            provider_session_id: Arc::from(session.factory.profile().session_id()),
            conversation: session.conversation.clone(),
            request_prefix: session.factory.profile().shared_prefix(),
            prompt_cache_key: Arc::from(session.factory.profile().prompt_cache_key()),
            preserve_inherited_delta: true,
            global_instructions: self.global_instructions.clone(),
            context_baseline: session.context.baseline(),
        })
    }

    fn fresh_session(&self, requested_workspace: Option<&str>) -> Result<ModelSessionState> {
        let workspace = requested_workspace.map_or_else(
            || self.context_source.resolve_workspace(None),
            |workspace| Ok(workspace.to_owned()),
        )?;
        let selected_agents_md = self
            .context_source
            .project_instructions(&workspace)
            .map(Arc::<str>::from);
        let tools = tool_runtime(&workspace, &self.config, &self.tools);
        let factory = self.attempt_factory(&tools)?;
        let context = ContextState::new(selected_agents_md, ContextBaseline::Missing);
        let canonical_context = context
            .capture(
                tools.working_directory(),
                tools.default_shell_name(),
                self.context_source.execution_environment(),
            )
            .full_item();
        Ok(ModelSessionState {
            workspace,
            tools,
            factory,
            conversation: ConversationState::empty(canonical_context),
            context,
            preserve_inherited_delta: false,
        })
    }

    fn attempt_factory(&self, tools: &ToolRuntime) -> Result<ResponsesAttemptFactory> {
        attempt_factory(
            &self.events,
            &self.transport_stats,
            &self.provider_session_id,
            &self.prompt_cache,
            self.model,
            tools,
            &self.config.base_system_prompt(),
        )
        .map(|factory| factory.with_transport_fallback(&self.transport_fallback))
    }

    fn responses_endpoint(&self) -> &str {
        match self.config.responses_transport {
            ResponsesTransport::WebSocket => &self.config.websocket_url,
            ResponsesTransport::Https => &self.config.api_base_url,
        }
    }
}

fn code_only_checkpoint(mut checkpoint: ModelCheckpoint, runtime: &ToolRuntime) -> ModelCheckpoint {
    if !runtime.is_code_only() {
        return checkpoint;
    }
    let has_schemas = checkpoint
        .conversation
        .managed
        .history()
        .any(|item| match item {
            ResponseItem::ToolSearchOutput { tools, .. } => !tools.is_empty(),
            ResponseItem::AdditionalTools { tools, .. } => !tools.is_empty(),
            _ => false,
        });
    if has_schemas {
        let mut history = checkpoint.conversation.flattened_history();
        clear_code_only_schemas(&mut history);
        checkpoint
            .conversation
            .managed
            .replace_prepared_history(history);
        checkpoint.preserve_inherited_delta = false;
    }
    checkpoint
}

fn clear_code_only_schemas(history: &mut [ResponseItem]) {
    for item in history {
        match item {
            ResponseItem::ToolSearchOutput { tools, .. } => tools.clear(),
            ResponseItem::AdditionalTools { tools, .. } => tools.clear(),
            _ => {}
        }
    }
}

pub(crate) fn prepare_checkpoint(
    checkpoint: ModelCheckpoint,
    config: &ModelConfig,
    tools: &Tools,
    context_source: ContextSource,
) -> PreparedCheckpoint {
    let runtime = tool_runtime(checkpoint.workspace(), config, tools);
    let selected_agents_md = context_source
        .project_instructions(checkpoint.workspace())
        .map(Arc::from);
    let checkpoint = code_only_checkpoint(checkpoint, &runtime);
    PreparedCheckpoint {
        checkpoint,
        runtime,
        context_source,
        selected_agents_md,
    }
}

pub(crate) fn prepare_resumed_checkpoint(
    mut checkpoint: ModelCheckpoint,
    config: &ModelConfig,
    tools: &Tools,
    session_id: &str,
    context_source: ContextSource,
) -> Result<PreparedCheckpoint> {
    if checkpoint.conversation.prepare_replay_images() {
        checkpoint.preserve_inherited_delta = false;
    }

    if !config.store_responses {
        checkpoint.conversation.reset_for_full_request();
    }
    checkpoint.global_instructions = context_source
        .global_instructions()
        .or(checkpoint.global_instructions);
    let runtime = tool_runtime(checkpoint.workspace(), config, tools);
    let (tool_specs, code_mode_tool_names) = model_tool_contract(&runtime, session_id);
    checkpoint.request_prefix = Arc::from(
        request_profile(
            session_id,
            checkpoint.prompt_cache_key(),
            tool_specs,
            code_mode_tool_names,
            &config.base_system_prompt(),
        )?
        .prefix()
        .to_vec(),
    );
    let selected_agents_md = context_source
        .project_instructions(checkpoint.workspace())
        .map(Arc::from);
    let checkpoint = code_only_checkpoint(checkpoint, &runtime);
    Ok(PreparedCheckpoint {
        checkpoint,
        runtime,
        context_source,
        selected_agents_md,
    })
}

pub(crate) fn prepare_history_checkpoint(
    resume: HistoryCheckpoint,
    config: &ModelConfig,
    tools: &Tools,
    session_id: &str,
    context_source: ContextSource,
) -> Result<PreparedCheckpoint> {
    let HistoryCheckpoint {
        workspace,
        provider_session_id,
        canonical_context,
        history,
        client_authored,
        prompt_cache_key,
        context_baseline,
        reasoning,
    } = resume;
    let selected_agents_md = context_source
        .project_instructions(&workspace)
        .map(Arc::from);
    let runtime = tool_runtime(&workspace, config, tools);
    let (tool_specs, code_mode_tool_names) = model_tool_contract(&runtime, session_id);
    let request_prefix = request_profile(
        session_id,
        prompt_cache_key.as_ref(),
        tool_specs,
        code_mode_tool_names,
        &config.base_system_prompt(),
    )?
    .prefix()
    .to_vec();
    let mut checkpoint = ModelCheckpoint::resume(
        workspace,
        provider_session_id,
        request_prefix,
        prompt_cache_key,
        canonical_context,
        history,
        client_authored,
        context_source.global_instructions(),
        context_baseline,
    )?;
    checkpoint.restore_reasoning(reasoning);
    let checkpoint = code_only_checkpoint(checkpoint, &runtime);
    Ok(PreparedCheckpoint {
        checkpoint,
        runtime,
        context_source,
        selected_agents_md,
    })
}

use crate::builtin_tools::tool_runtime;
