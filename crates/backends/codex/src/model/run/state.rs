use super::*;

pub(super) struct PreparedSession {
    pub(super) workspace: String,
    pub(super) config: Arc<ModelConfig>,
    pub(super) model: Model,
    pub(super) request_id: String,
    pub(super) catalog: Vec<u64>,
    pub(super) tools: ToolRuntime,
    pub(super) factory: ResponsesAttemptFactory,
}

pub(super) struct ModelSessionState {
    pub(super) workspace: String,
    pub(super) tools: ToolRuntime,
    pub(super) factory: ResponsesAttemptFactory,
    pub(super) conversation: ConversationState,
    pub(super) context: ContextState,
    pub(super) preserve_inherited_delta: bool,
}

struct ExecHistory(ResponseHistory);

impl HistorySource for ExecHistory {
    fn items(&self) -> Vec<ResponseItem> {
        self.0.iter().cloned().collect()
    }
}

impl ModelSessionState {
    pub(super) fn exec_history(&mut self) -> Arc<SharedHistory> {
        self.conversation.commit_tail();
        Arc::new(SharedHistory::from_source(ExecHistory(
            self.conversation.shared_history(),
        )))
    }

    pub(super) fn validate_workspace(&self, requested: Option<&str>) -> Result<()> {
        let Some(requested) = requested else {
            return Ok(());
        };
        if requested != self.workspace {
            return Err(NanocodeError::WorkspaceChanged {
                current: self.workspace.clone(),
                requested: requested.to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct ContinuationPolicy {
    pub(super) model: Model,
    pub(super) thinking: Thinking,
    pub(super) service_tier: ServiceTier,
    pub(super) reasoning_effort_updates: bool,
}

#[derive(Clone)]
pub(super) struct ConversationState {
    pub(super) canonical_context: Arc<ResponseItem>,
    pub(super) managed: ManagedSessionState,
    pub(super) continuation_policy: Option<ContinuationPolicy>,
    pub(super) reasoning: crate::reasoning::ReasoningState,
}

impl ConversationState {
    pub(super) fn empty(canonical_context: ResponseItem) -> Self {
        Self {
            canonical_context: Arc::new(canonical_context),
            managed: ManagedSessionState::new(Vec::new()),
            continuation_policy: None,
            reasoning: Default::default(),
        }
    }

    pub(super) fn new(history: Vec<ResponseItem>) -> Result<Self> {
        let canonical_context = history
            .iter()
            .find(|item| item.is_user_message())
            .cloned()
            .ok_or(NanocodeError::MalformedResponse {
                detail: "task input did not include initial context",
            })?;
        Ok(Self {
            canonical_context: Arc::new(canonical_context),
            managed: ManagedSessionState::new(history),
            continuation_policy: None,
            reasoning: Default::default(),
        })
    }

    pub(super) fn resume(
        mut canonical_context: ResponseItem,
        history: Vec<ResponseItem>,
    ) -> Result<Self> {
        if !canonical_context.is_user_message() {
            return Err(NanocodeError::InvalidCheckpoint(
                "canonical context must be a user message".to_owned(),
            ));
        }
        assign_missing_response_item_id(&mut canonical_context);
        let managed = ManagedSessionState::resume(history)
            .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?;
        let mut state = Self {
            canonical_context: Arc::new(canonical_context),
            managed,
            continuation_policy: None,
            reasoning: Default::default(),
        };
        state.prepare_replay_images();
        Ok(state)
    }

    pub(super) fn prepare_replay_images(&mut self) -> bool {
        let mut history = self.managed.flattened_history();
        let history_changed = nanocode_code_mode::image::prepare_history_images(&mut history);
        let context_changed = nanocode_code_mode::image::prepare_history_images(
            std::slice::from_mut(Arc::make_mut(&mut self.canonical_context)),
        );
        let changed = history_changed || context_changed;
        if changed {
            self.managed.replace_prepared_history(history);
        }
        changed
    }

    pub(super) fn flattened_history(&self) -> Vec<ResponseItem> {
        self.managed.flattened_history()
    }

    pub(super) fn clear_delta(&mut self) {
        self.managed.clear_delta();
    }

    pub(super) fn append(&mut self, items: impl IntoIterator<Item = ResponseItem>) {
        self.managed.append(items);
    }

    pub(super) fn append_client(&mut self, items: impl IntoIterator<Item = ResponseItem>) {
        self.managed.append_client(items);
    }

    pub(super) const fn observe_server_reasoning(&mut self, included: bool) {
        self.managed.observe_server_reasoning(included);
    }

    pub(super) fn active_context_tokens(&self) -> u64 {
        self.managed.active_context_tokens()
    }

    pub(super) fn shared_history(&self) -> ResponseHistory {
        self.managed.shared_history()
    }

    pub(super) fn set_previous_response_id(&mut self, response_id: impl Into<String>) {
        self.managed.set_previous_response_id(response_id);
    }

    pub(super) fn install_pre_turn_compaction(
        &mut self,
        item: ResponseItem,
        request_prefix: &[ResponseItem],
    ) {
        self.managed.install_compaction(item, [], request_prefix);
        self.reasoning.compacted();
    }

    pub(super) fn install_mid_turn_compaction(
        &mut self,
        item: ResponseItem,
        canonical_developer_context: ResponseItem,
        canonical_context: ResponseItem,
        request_prefix: &[ResponseItem],
    ) {
        self.canonical_context = Arc::new(canonical_context.clone());
        let initial_context = [canonical_developer_context, canonical_context];
        self.managed
            .install_compaction(item, initial_context, request_prefix);
        self.reasoning.compacted();
    }

    pub(super) fn append_canonical_context(
        &mut self,
        canonical_developer_context: ResponseItem,
        canonical_context: ResponseItem,
    ) {
        self.canonical_context = Arc::new(canonical_context.clone());
        self.managed
            .append([canonical_developer_context, canonical_context]);
    }

    pub(super) fn set_canonical_context(&mut self, canonical_context: ResponseItem) {
        self.canonical_context = Arc::new(canonical_context);
    }

    pub(super) fn reset_for_full_request(&mut self) {
        self.managed.reset_for_full_request();
    }

    pub(super) fn prepare_request_policy(&mut self, policy: ContinuationPolicy) {
        if self.continuation_policy.is_some_and(|previous| {
            previous.model != policy.model
                || previous.service_tier != policy.service_tier
                || previous.reasoning_effort_updates != policy.reasoning_effort_updates
                || (previous.thinking != policy.thinking && !policy.reasoning_effort_updates)
        }) {
            self.reset_for_full_request();
        }
        self.continuation_policy = Some(policy);
    }

    pub(super) fn commit(&mut self) -> Result<()> {
        self.managed
            .commit()
            .map_err(|_| NanocodeError::MalformedResponse {
                detail: "completed turn did not have a response ID",
            })
    }

    pub(super) fn commit_interrupted(&mut self) {
        self.managed.commit_interrupted();
    }

    pub(super) fn commit_tail(&mut self) {
        self.managed.commit_tail();
    }
}
