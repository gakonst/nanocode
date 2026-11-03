use super::*;
use crate::model::telemetry::UsageCost;

#[derive(Deserialize, Serialize)]
struct CurrentExecution {
    phase: ExecutionPhase,
    #[serde(default)]
    instruction_revision: Option<u64>,
    workspace: String,
    canonical_context: ResponseItem,
    #[serde(default)]
    client_authored: std::collections::BTreeSet<String>,
    context_baseline: ContextBaseline,
    #[serde(default)]
    reasoning: crate::reasoning::ReasoningState,
    #[serde(default)]
    context_usage: Option<Usage>,
    #[serde(default)]
    server_reasoning_included: bool,
    #[serde(default)]
    context_usage_is_estimate: bool,
    prompt_cache_key: String,
    model: String,
    effort: Thinking,
    #[serde(flatten, with = "crate::service_tier_serde")]
    service_tier: ServiceTier,
    reasoning_mode: String,
    model_id_prefix: Option<String>,
    store_responses: bool,
    #[serde(with = "stored_stats")]
    stats: RunStats,
    #[serde(default)]
    operation_started_at: Option<Duration>,
    usage_reported: bool,
    usage_cost: Option<crate::client::pricing::EstimatedUsdCost>,
    warmup_reported: bool,
    warmup_cost: Option<crate::client::pricing::EstimatedUsdCost>,
    context_window_tokens: u64,
    force_compaction: bool,
    tool_call_indices: HashMap<Box<str>, u32>,
}

mod stored_stats {
    use serde::{Deserialize as _, Deserializer, Serialize as _, Serializer, de, ser};
    use serde_json::Value;

    use super::RunStats;

    pub(super) fn serialize<S: Serializer>(
        stats: &RunStats,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut value = serde_json::to_value(stats).map_err(ser::Error::custom)?;
        prune(&mut value);
        value.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<RunStats, D::Error> {
        let mut stats = serde_json::to_value(RunStats::default()).map_err(de::Error::custom)?;
        merge(&mut stats, Value::deserialize(deserializer)?);
        serde_json::from_value(stats).map_err(de::Error::custom)
    }

    fn prune(value: &mut Value) -> bool {
        match value {
            Value::Null => true,
            Value::Number(number) => number.as_f64() == Some(0.0),
            Value::Object(fields) => {
                fields.retain(|_, field| !prune(field));
                fields.is_empty()
            }
            Value::Bool(_) | Value::String(_) | Value::Array(_) => false,
        }
    }

    fn merge(base: &mut Value, overlay: Value) {
        match (base, overlay) {
            (Value::Object(base), Value::Object(overlay)) => {
                for (key, value) in overlay {
                    match base.get_mut(&key) {
                        Some(field) => merge(field, value),
                        None => {
                            base.insert(key, value);
                        }
                    }
                }
            }
            (base, overlay) => *base = overlay,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub(super) enum ExecutionPhase {
    Warmup,
    PrepareTurn,
    Generate,
    Compact,
}

impl ModelRun {
    pub(super) async fn restore_execution(
        &mut self,
        requested_workspace: Option<&str>,
        logical_turn: u64,
    ) -> Result<Option<(ModelSessionState, ExecutionPhase)>> {
        let Some(steps) = &self.execution_steps else {
            return Ok(None);
        };
        let Some((mut saved, history, prefix)) = steps.continuation::<CurrentExecution>().await?
        else {
            return Ok(None);
        };
        if saved.stats.model_calls == u32::MAX {
            return Err(NanocodeError::InvalidExecutionPolicy(
                "invalid execution continuation".into(),
            ));
        }
        saved.reasoning.validate(&history)?;
        let mut session = self.empty_session(Some(&saved.workspace))?;
        session.validate_workspace(requested_workspace)?;
        let (history, prefix) = if session.tools.is_code_only() {
            let mut history = history;
            clear_code_only_schemas(&mut history);

            let mut prefix = prefix;

            let tools_at = prefix
                .iter()
                .position(|item| matches!(item, ResponseItem::AdditionalTools { .. }))
                .unwrap_or(prefix.len());
            prefix.retain(|item| !matches!(item, ResponseItem::AdditionalTools { .. }));
            clear_code_only_schemas(&mut prefix);
            let current_tools = session
                .factory
                .profile()
                .prefix()
                .iter()
                .filter(|item| matches!(item, ResponseItem::AdditionalTools { .. }))
                .cloned()
                .collect::<Vec<_>>();
            prefix.splice(tools_at..tools_at, current_tools);
            (history, prefix)
        } else {
            (history, prefix)
        };
        self.model = saved
            .model
            .parse::<crate::Model>()
            .map_err(NanocodeError::InvalidExecutionPolicy)?;
        self.thinking = saved.effort;
        self.service_tier = saved.service_tier;
        let config = Arc::make_mut(&mut self.config);
        config.reasoning_mode = saved
            .reasoning_mode
            .parse::<crate::ReasoningMode>()
            .map_err(NanocodeError::InvalidExecutionPolicy)?;
        config.model_id_prefix = saved.model_id_prefix.clone().map(Arc::from);
        config.store_responses = saved.store_responses;
        config.context_window_tokens = saved.context_window_tokens;
        self.force_compaction = saved.force_compaction;
        self.instruction_revision = saved.instruction_revision;
        session.factory = session
            .factory
            .with_request_content(
                saved.prompt_cache_key,
                prefix.into(),
                saved.model_id_prefix,
                saved
                    .reasoning_mode
                    .parse::<crate::ReasoningMode>()
                    .map_err(NanocodeError::InvalidExecutionPolicy)?,
                saved.store_responses,
            )
            .for_logical_turn(logical_turn);
        session.conversation = if history.is_empty() && saved.phase == ExecutionPhase::Compact {
            ConversationState::empty(saved.canonical_context)
        } else {
            ConversationState::resume(saved.canonical_context, history)?
        };
        session
            .conversation
            .managed
            .restore_client_authored(saved.client_authored);
        session.conversation.managed.restore_context_usage(
            saved.context_usage.as_ref(),
            saved.server_reasoning_included,
            saved.context_usage_is_estimate,
        );
        session.conversation.reasoning = saved.reasoning;
        session
            .conversation
            .prepare_request_policy(self.continuation_policy());

        session.conversation.reset_for_full_request();
        session.context = ContextState::new(
            self.context_source
                .project_instructions(&saved.workspace)
                .map(Arc::<str>::from),
            saved.context_baseline,
        );
        session.context.restore_host_instructions(
            session.conversation.managed.history(),
            &self.config.host_instructions(),
        );
        saved.stats.last_response_id = None;
        saved.stats.usage.cost = UsageCost::restored(saved.usage_reported, saved.usage_cost);
        saved.stats.warmup_usage.cost =
            UsageCost::restored(saved.warmup_reported, saved.warmup_cost);
        self.stats = saved.stats;
        if let Some(origin) = saved.operation_started_at {
            self.elapsed_before_attempt = self
                .operation_started_at
                .and_then(|started_at| started_at.checked_sub(origin))
                .unwrap_or_default();
            self.operation_started_at = Some(origin);
        }
        self.tool_call_indices = saved.tool_call_indices;
        self.active_tools
            .as_ref()
            .expect("restored tools")
            .begin_turn();
        Ok(Some((session, saved.phase)))
    }

    pub(super) fn restore_runtime(
        &mut self,
        configured: (Arc<ModelConfig>, Model),
        logical_turn: u64,
    ) -> Result<()> {
        let recovered_settings = !Arc::ptr_eq(&self.config, &configured.0);
        self.config = configured.0;
        self.model = configured.1;
        if !recovered_settings {
            return Ok(());
        }

        if let Some(mut session) = self.session.take() {
            let current = self.attempt_factory(&session.tools)?;
            session.factory = session
                .factory
                .with_request_content(
                    current.profile().prompt_cache_key().to_owned(),
                    current.profile().shared_prefix(),
                    self.config.model_id_prefix.as_deref().map(str::to_owned),
                    self.config.reasoning_mode,
                    self.config.store_responses,
                )
                .for_logical_turn(logical_turn);
            self.session = Some(session);
        }
        Ok(())
    }

    pub(super) fn record_transport(&mut self) {
        self.stats
            .apply_transport(self.transport_stats.since(self.transport_baseline));
        self.transport_baseline = self.transport_stats.snapshot();
    }

    pub(super) async fn retain_execution(
        &mut self,
        session: &mut ModelSessionState,
        phase: ExecutionPhase,
    ) -> Result<()> {
        self.retain_then_begin::<()>(session, phase, None)
            .await
            .map(drop)
    }

    pub(super) async fn retain_before_model_call(
        &mut self,
        session: &mut ModelSessionState,
        call_index: u32,
    ) -> Result<Option<crate::agent::ExecutionStep<RecordedModelResult>>> {
        self.retain_then_begin(
            session,
            ExecutionPhase::Generate,
            Some((model_step_id(call_index), "model_call")),
        )
        .await
    }

    async fn retain_then_begin<O: serde::de::DeserializeOwned>(
        &mut self,
        session: &mut ModelSessionState,
        phase: ExecutionPhase,
        next_step: Option<(String, &str)>,
    ) -> Result<Option<crate::agent::ExecutionStep<O>>> {
        session.conversation.commit_tail();
        if self.execution_steps.is_none() {
            return Ok(None);
        }
        self.record_transport();
        let steps = self.execution_steps.as_ref().expect("durable execution");
        let saved = CurrentExecution {
            phase,
            instruction_revision: self.instruction_revision,
            workspace: session.workspace.clone(),
            canonical_context: (*session.conversation.canonical_context).clone(),
            client_authored: session.conversation.managed.client_authored().clone(),
            context_baseline: session.context.baseline(),
            reasoning: session.conversation.reasoning.clone(),
            context_usage: session.conversation.managed.context_usage().0.cloned(),
            server_reasoning_included: session.conversation.managed.context_usage().1,
            context_usage_is_estimate: session.conversation.managed.context_usage_is_estimate(),
            prompt_cache_key: session.factory.profile().prompt_cache_key().to_owned(),
            model: self.model.as_str().to_owned(),
            effort: self.thinking,
            service_tier: self.service_tier,
            reasoning_mode: self.config.reasoning_mode.as_str().to_owned(),
            model_id_prefix: self.config.model_id_prefix.as_deref().map(str::to_owned),
            store_responses: self.config.store_responses,
            stats: self.stats.clone(),
            operation_started_at: self.operation_started_at,
            usage_reported: self.stats.usage.cost.reported(),
            usage_cost: self.stats.usage.cost.estimated().cloned(),
            warmup_reported: self.stats.warmup_usage.cost.reported(),
            warmup_cost: self.stats.warmup_usage.cost.estimated().cloned(),
            context_window_tokens: self.config.context_window_tokens,
            force_compaction: self.force_compaction,
            tool_call_indices: self.tool_call_indices.clone(),
        };
        if phase == ExecutionPhase::PrepareTurn && next_step.is_none() {
            steps
                .advance_deferred(
                    &saved,
                    session.conversation.shared_history(),
                    session.factory.profile().shared_prefix(),
                )
                .await?;
            return Ok(None);
        }
        steps
            .advance(
                &saved,
                session.conversation.shared_history(),
                session.factory.profile().shared_prefix(),
                next_step.map(|(step_id, kind)| (step_id, kind, &())),
            )
            .await
    }
}
