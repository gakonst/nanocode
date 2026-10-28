use super::*;
use crate::{
    MessageResponse,
    execution::{BeginRequest, EncodedState, JournalWrite, Settlement},
};
use nanocode_durability::{EncodedPayload, Sequence, SequenceLog};
use serde_json::value::RawValue;
use std::collections::BTreeSet;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    provider: String,
    version: u32,
    pub(super) conversation: Conversation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) developer_notices: Vec<(String, String)>,
    pub(super) discovered: HashSet<String>,
    pub(super) tasks: Option<Value>,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) workspace: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) lineage: Option<Lineage>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) conversation_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) effort: Option<crate::Effort>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) thinking_disabled: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) fast_mode: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            provider: "claude".into(),
            version: 1,
            conversation: Conversation::default(),
            developer_notices: Vec::new(),
            discovered: HashSet::new(),
            tasks: None,
            model: None,
            workspace: None,
            lineage: None,
            conversation_id: None,
            effort: None,
            thinking_disabled: false,
            fast_mode: false,
        }
    }
}
impl Snapshot {
    pub(super) fn decode(value: Value) -> Result<Self> {
        serde_json::from_value::<Self>(value)
            .map_err(provider_error)?
            .validated()
    }
    pub(super) fn validated(self) -> Result<Self> {
        if self.provider != "claude" || self.version != 1 {
            return Err(NanocodeError::InvalidCheckpoint(
                "unsupported Claude checkpoint version/provider".into(),
            ));
        }
        Ok(self)
    }

    pub(super) fn session_context(&self, workspace: String) -> AgentSessionContext {
        use nanocode_types::responses::{ContentItem, MessageRole, ResponseItem};
        let mut history =
            self.conversation
                .packed_messages()
                .into_iter()
                .map(|message| {
                    ResponseItem::message(
                        match message.role {
                            Role::User => MessageRole::User,
                            Role::Assistant => MessageRole::Assistant,
                        },
                        message.into_message().content.into_iter().filter_map(
                            |block| match block {
                                ContentBlock::Text { text, .. } => Some(ContentItem::InputText {
                                    text: text.into_boxed_str(),
                                }),
                                _ => None,
                            },
                        ),
                    )
                })
                .collect::<Vec<_>>();
        history.extend(self.developer_notices.iter().map(|(_, text)| {
            ResponseItem::message(
                MessageRole::Developer,
                [ContentItem::InputText {
                    text: text.clone().into_boxed_str(),
                }],
            )
        }));
        AgentSessionContext::from_backend(workspace, history)
    }

    pub(super) const fn has_conversation(&self) -> bool {
        !self.conversation.messages.is_empty() || !self.conversation.summary.is_empty()
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    #[serde(skip)]
    pub(super) recovered_code_index: Option<u32>,
    pub(super) snapshot: Snapshot,
    pub(super) template: MessagesRequest,
    #[serde(default)]
    pub(super) lifecycle_turn_id: String,
    #[serde(default)]
    pub(super) stop_hook_active: bool,
    #[serde(default)]
    pub(super) instruction_revision: Option<u64>,

    #[serde(default)]
    pub(super) dynamic_tool_names: HashSet<String>,
    #[serde(default)]
    pub(super) wire_profile: Option<crate::FrozenWireProfile>,
    pub(super) threshold: u64,
    pub(super) parallel: bool,
    pub(super) tool_search: bool,
    pub(super) operation: Option<String>,
    pub(super) prepared: bool,
    #[serde(default)]
    pub(super) developer_notice_ids: HashSet<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) frozen_prompt: Option<Prompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) pending_base: Option<u32>,
    pub(super) pending: Vec<HistoryMessage>,
    pub(super) usage: Usage,
    pub(super) index: u32,
    #[serde(default)]
    pub(super) steers: u32,

    #[serde(default)]
    pub(super) model_step_offset: u32,
    #[serde(default)]
    pub(super) model_receipt_start: Option<u32>,

    #[serde(default)]
    pub(super) context_recovery_attempted: bool,
    #[serde(default)]
    pub(super) output_continuations: u32,
}
impl Cursor {
    pub(super) fn host_context(&self, state: &State) -> Option<Arc<str>> {
        state
            .host_context
            .clone()
            .or_else(|| self.operation.as_deref().map(Arc::from))
    }
    pub(super) fn effect<'a>(&'a self, state: &'a State, step: &str) -> Option<Effect<'a>> {
        Some(Effect {
            policy: state.policy.as_deref()?,
            operation: self.operation.as_deref()?,
            step: step.to_owned(),
            model_call: step
                .strip_prefix("model-")
                .and_then(|index| index.parse::<u32>().ok())
                .zip(self.model_receipt_start)
                .is_some_and(|(index, start)| index >= start),
        })
    }
}
pub(super) struct Effect<'a> {
    policy: &'a dyn ClaudeExecutionPolicy,
    operation: &'a str,
    step: String,
    model_call: bool,
}
impl Effect<'_> {
    pub(super) fn scoped(&self, scope: &str) -> Self {
        Effect {
            policy: self.policy,
            operation: self.operation,
            step: format!("{scope}-{}", self.step),
            model_call: self.model_call,
        }
    }

    pub(super) async fn begin_model(
        &self,
        wire: &crate::PreparedWire,
        writes: Vec<JournalWrite>,
    ) -> Result<Step> {
        let request = BeginRequest {
            step_id: self.step.clone(),
            kind: if self.model_call {
                "model_call"
            } else {
                "model"
            }
            .to_owned(),
            input: wire.durable_input().map_err(provider_error)?,
            wire: Some(wire),
        };
        self.admit(writes, request).await
    }
    pub(super) async fn begin_tool(
        &self,
        input: Box<RawValue>,
        writes: Vec<JournalWrite>,
    ) -> Result<Step> {
        let request = BeginRequest {
            step_id: self.step.clone(),
            kind: "tool".to_owned(),
            input,
            wire: None,
        };
        self.admit(writes, request).await
    }
    async fn admit(&self, writes: Vec<JournalWrite>, request: BeginRequest<'_>) -> Result<Step> {
        self.policy
            .commit_writes(self.operation.to_owned(), writes, Some(request))
            .await?
            .ok_or_else(|| recovery_error("the Claude step admission returned no outcome"))
    }
    pub(super) async fn complete(&self, output: Value) -> Result<()> {
        self.policy
            .complete_step(self.operation.to_owned(), self.step.clone(), output)
            .await
    }
    pub(super) fn completion(&self, output: &impl Serialize) -> Result<JournalWrite> {
        Ok(JournalWrite::Complete {
            step_id: self.step.clone(),
            output: serde_json::value::to_raw_value(output).map_err(provider_error)?,
            records: Vec::new(),
        })
    }

    pub(super) fn model_completion(
        &self,
        response: &mut MessageResponse,
        paged: bool,
    ) -> Result<JournalWrite> {
        #[derive(Serialize)]
        struct Receipt<'a> {
            content_record: EncodedPayload,
            #[serde(flatten)]
            response: &'a MessageResponse,
        }
        if !paged {
            return self.completion(response);
        }
        let message = Message {
            role: Role::Assistant,
            content: std::mem::take(&mut response.content),
        };
        let record =
            serde_json::to_string(&message).map(|json| EncodedPayload::from_json(json.into()));
        let output = record.as_ref().map_err(provider_error).and_then(|record| {
            serde_json::value::to_raw_value(&Receipt {
                content_record: record.reference(),
                response,
            })
            .map_err(provider_error)
        });
        response.content = message.content;
        Ok(JournalWrite::Complete {
            step_id: self.step.clone(),
            output: output?,
            records: vec![record.map_err(provider_error)?],
        })
    }
}

#[derive(Default)]
pub(super) struct Journal {
    operation: Option<String>,
    writes: Vec<JournalWrite>,
}
impl Journal {
    fn advance(&mut self, state: EncodedState) {
        self.writes
            .retain(|write| !matches!(write, JournalWrite::Advance(_)));
        self.writes.push(JournalWrite::Advance(state));
    }
    pub(super) fn complete(&mut self, write: JournalWrite) {
        self.writes.push(write);
    }
    pub(super) fn take(&mut self) -> Vec<JournalWrite> {
        std::mem::take(&mut self.writes)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CodeOnlyUpgrade {
    pub(super) code_only_tools: Vec<ClaudeToolSpec>,
    pub(super) notice: String,
}
impl CodeOnlyUpgrade {
    pub(super) fn new(code_only_tools: Vec<ClaudeToolSpec>, tools_disabled: bool) -> Self {
        Self {
            code_only_tools,
            notice: if tools_disabled {
                "Harness recovery notice: the interrupted summary request was reissued with the current tool catalog. Produce the requested text-only summary."
            } else {
                "Harness recovery notice: this operation was upgraded to Code Mode. The preceding direct-tool model request was retired with outcome unknown; provider-side effects may already have occurred. Do not automatically repeat them. Reconcile effects before continuing through exec and wait."
            }.into(),
        }
    }
}
pub(super) fn is_code_only_catalog(tools: &[ClaudeToolSpec]) -> bool {
    tools.len() == 2
        && ["exec", "wait"].iter().all(|name| {
            tools
                .iter()
                .any(|tool| matches!(tool, ClaudeToolSpec::Client(tool) if tool.name == *name))
        })
}
impl State {
    pub(super) fn code_only_tools(&self) -> Vec<ClaudeToolSpec> {
        self.available_tools().into_iter().filter(|tool| {
            matches!(tool, ClaudeToolSpec::Client(tool) if tool.name == "exec" || tool.name == "wait")
        }).collect()
    }
    pub(super) fn classify_code_only_tools(&self, cursor: &mut Cursor) {
        cursor.dynamic_tool_names = self
            .dynamic_catalog()
            .into_iter()
            .filter_map(|(definition, _)| {
                cursor.template.tools.iter().any(|tool| {
                    matches!(tool, ClaudeToolSpec::Client(admitted) if admitted == &definition)
                }).then_some(definition.name)
            })
            .collect();
    }
    #[cfg_attr(not(not(target_family = "wasm")), allow(clippy::missing_const_for_fn))]
    fn task_snapshot(&self) -> Result<Option<Value>> {
        #[cfg(not(target_family = "wasm"))]
        {
            self.task_board
                .as_ref()
                .map(|tasks| tasks.snapshot().map_err(provider_error))
                .transpose()
        }
        #[cfg(not(not(target_family = "wasm")))]
        {
            Ok(None)
        }
    }
    fn restore_tasks(&self, tasks: Option<Value>) -> Result<()> {
        if let Some(tasks) = tasks {
            #[cfg(not(target_family = "wasm"))]
            self.task_board
                .as_ref()
                .ok_or_else(|| unsupported("Claude task checkpoint requires a task board"))?
                .restore(tasks)
                .map_err(provider_error)?;
            #[cfg(not(not(target_family = "wasm")))]
            {
                let _ = tasks;
                return Err(unsupported(
                    "Claude task checkpoint restoration requires a native target with tools and a task board",
                ));
            }
        }
        Ok(())
    }
    pub(super) async fn snapshot(&self, conversation: &Conversation) -> Result<Snapshot> {
        Ok(Snapshot {
            conversation: conversation.clone(),
            developer_notices: Vec::clone(&self.developer_notices.get()),
            discovered: HashSet::clone(&self.discovered.get()),
            tasks: self.task_snapshot()?,
            model: Some(self.model()),
            workspace: Some(self.workspace()),
            lineage: Some(self.lineage.clone()),
            conversation_id: Some(self.conversation_id.clone()),
            effort: self.effort(),
            thinking_disabled: self.thinking_disabled.load(Ordering::SeqCst),
            fast_mode: self.fast_mode.load(Ordering::SeqCst),
            ..Snapshot::default()
        })
    }
    async fn restore_snapshot(
        &self,
        conversation: &mut Conversation,
        snapshot: Snapshot,
    ) -> Result<()> {
        self.restore_tasks(snapshot.tasks)?;
        self.discovered.set(snapshot.discovered);
        self.developer_notices.set(snapshot.developer_notices);
        *conversation = snapshot.conversation;
        Ok(())
    }
    async fn merge_developer_notices(
        &self,
        policy: &dyn ClaudeExecutionPolicy,
        operation: &str,
        selected: &mut HashSet<String>,
    ) -> Result<()> {
        let notices = policy.developer_notices(operation.to_owned()).await?;
        for notice in &notices {
            selected.remove(&notice.0);
        }
        self.developer_notices.update(|queued| {
            for notice in &notices {
                if !queued.iter().any(|(id, _)| id == &notice.0) {
                    queued.push(notice.clone());
                }
            }
        });
        Ok(())
    }

    pub(super) async fn cursor(
        &self,
        conversation: &mut Conversation,
        operation: Option<&str>,
        speed: Option<crate::Speed>,
        prompt: Option<&Prompt>,
        mut developer_notice_ids: HashSet<String>,
        journal: &mut Journal,
    ) -> Result<Cursor> {
        let update = self.persistence_update.enter().await;
        if let (Some(policy), Some(operation)) = (&self.policy, operation)
            && let Some(value) = policy.continuation(operation.to_owned()).await?
        {
            let mut cursor: Cursor = serde_json::from_value(value).map_err(recovery_error)?;
            if let Some(base) = cursor.pending_base.take() {
                let prefix = usize::try_from(base)
                    .ok()
                    .and_then(|base| cursor.snapshot.conversation.messages.get(..base))
                    .ok_or_else(|| {
                        recovery_error(
                            "Claude continuation pending prefix exceeds its conversation",
                        )
                    })?;
                cursor.pending = prefix
                    .iter()
                    .cloned()
                    .chain(std::mem::take(&mut cursor.pending))
                    .collect();
            }
            cursor.recovered_code_index = Some(cursor.index);

            if cursor.model_receipt_start.is_none() {
                cursor.model_receipt_start = Some(cursor.index.saturating_add(1));
            }
            if cursor.operation.as_deref() != Some(operation)
                || cursor.snapshot.provider != "claude"
                || cursor.snapshot.version != 1
            {
                return Err(recovery_error("invalid Claude execution continuation"));
            }

            self.restore_snapshot(conversation, std::mem::take(&mut cursor.snapshot))
                .await
                .map_err(recovery_error)?;
            self.merge_developer_notices(
                policy.as_ref(),
                operation,
                &mut cursor.developer_notice_ids,
            )
            .await?;
            journal.operation.clone_from(&cursor.operation);
            return Ok(cursor);
        }
        if let (Some(policy), Some(operation)) = (&self.policy, operation) {
            self.merge_developer_notices(policy.as_ref(), operation, &mut developer_notice_ids)
                .await?;
        }
        drop(update);
        let template = self.request_template(speed);
        let static_names = self
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<HashSet<_>>();
        let dynamic_tool_names = template
            .tools
            .iter()
            .filter_map(|tool| match tool {
                ClaudeToolSpec::Client(tool) if !static_names.contains(tool.name.as_str()) => {
                    Some(tool.name.clone())
                }
                _ => None,
            })
            .collect();
        let wire_profile = self
            .client
            .freeze_wire_profile(template.cache_control.is_some());
        let mut cursor = Cursor {
            recovered_code_index: None,
            lifecycle_turn_id: candidate_id("lifecycle"),
            stop_hook_active: false,
            instruction_revision: None,
            snapshot: Snapshot::default(),
            template,
            dynamic_tool_names,
            wire_profile: Some(wire_profile),
            threshold: self.compaction_threshold(),
            parallel: self.parallel_tools,
            tool_search: self.client_tool_search,
            operation: operation.map(str::to_owned),
            prepared: false,
            developer_notice_ids,
            frozen_prompt: prompt
                .filter(|prompt| {
                    matches!(
                        prompt.instruction,
                        nanocode_agent_core::input::PromptInput::Content(_)
                    )
                })
                .cloned(),
            pending_base: None,
            pending: Vec::new(),
            usage: Usage::default(),
            index: 0,
            steers: 0,
            model_step_offset: 1,
            model_receipt_start: Some(0),
            context_recovery_attempted: false,
            output_continuations: 0,
        };

        #[cfg(not(target_family = "wasm"))]
        if self.policy.is_some() && self.task_board.is_some() {
            cursor.parallel = false;
        }
        self.defer_cursor(&mut cursor, conversation, journal)
            .await?;
        Ok(cursor)
    }

    pub(super) fn persists(&self, cursor: &Cursor) -> bool {
        self.policy.is_some() && cursor.operation.is_some()
    }

    pub(super) fn pages_messages(&self) -> bool {
        self.policy
            .as_ref()
            .is_some_and(|policy| policy.pages_messages())
    }

    pub(super) fn retain_pending(&self, cursor: &mut Cursor, pending: &[HistoryMessage]) {
        if self.persists(cursor) {
            cursor.pending = pending.to_vec();
        }
    }
    pub(super) async fn defer_cursor(
        &self,
        cursor: &mut Cursor,
        conversation: &mut Conversation,
        journal: &mut Journal,
    ) -> Result<()> {
        let _update = self.persistence_update.enter().await;
        self.defer_cursor_locked(cursor, conversation, journal)
            .await
    }
    pub(super) async fn defer_cursor_locked(
        &self,
        cursor: &mut Cursor,
        conversation: &mut Conversation,
        journal: &mut Journal,
    ) -> Result<()> {
        if self.code_only && !is_code_only_catalog(&cursor.template.tools) {
            cursor.template.tools = self.code_only_tools();
            self.classify_code_only_tools(cursor);
            cursor.tool_search = false;
        }

        if self.persists(cursor) {
            cursor.snapshot = self.snapshot(&Conversation::default()).await?;
            cursor.snapshot.conversation = std::mem::take(conversation);
            let mut paging = self.paging.take().unwrap_or_default();
            let encoded = encode_cursor(cursor, self.pages_messages(), &mut paging);
            self.paging.put(paging);
            *conversation = std::mem::take(&mut cursor.snapshot).conversation;
            journal.operation.clone_from(&cursor.operation);
            journal.advance(encoded?);
        }
        Ok(())
    }
    pub(super) async fn flush_for_lifecycle(
        &self,
        event: &crate::ClaudeLifecycleEvent,
        journal: &mut Journal,
    ) -> Result<()> {
        if self
            .tool_hooks
            .iter()
            .any(|hook| hook.handles_lifecycle(event))
        {
            self.flush_journal(journal).await?;
        }
        Ok(())
    }
    pub(super) async fn defer_journal(&self, journal: &mut Journal) -> Result<()> {
        let (Some(policy), Some(operation)) = (&self.policy, journal.operation.clone()) else {
            return Ok(());
        };
        let writes = journal.take();
        if writes.is_empty() {
            return Ok(());
        }
        let advanced = writes
            .iter()
            .any(|write| matches!(write, JournalWrite::Advance(_)));
        policy.defer_writes(operation, writes).await?;
        if advanced {
            self.persistence_ready.notify_waiters();
        }
        Ok(())
    }
    pub(super) async fn flush_journal(&self, journal: &mut Journal) -> Result<()> {
        let (Some(policy), Some(operation)) = (&self.policy, journal.operation.clone()) else {
            return Ok(());
        };
        let writes = journal.take();
        if writes.is_empty() {
            return Ok(());
        }
        let advanced = writes
            .iter()
            .any(|write| matches!(write, JournalWrite::Advance(_)));
        policy.commit_writes(operation, writes, None).await?;
        if advanced {
            self.persistence_ready.notify_waiters();
        }
        Ok(())
    }
    pub(super) async fn settle(
        &self,
        conversation: &mut Conversation,
        request: &BackendPrompt,
        result: &Result<TurnResult>,
    ) -> Result<()> {
        let _update = self.persistence_update.enter().await;
        let (Some(policy), Some(operation)) = (&self.policy, &request.request_id) else {
            return Ok(());
        };

        if result
            .as_ref()
            .err()
            .is_some_and(|error| error.execution_policy_disposition().is_some())
        {
            self.stopped.store(true, Ordering::SeqCst);
            return Ok(());
        }
        if matches!(result, Err(NanocodeError::TurnCancelled))
            && self.closing.load(Ordering::SeqCst)
        {
            let released = policy.release(operation.clone()).await;
            if let Some(turn) = self.steering.acquire().await.get_mut(&request.key) {
                turn.operation = None;
            }
            self.persistence_ready.notify_waiters();
            return released;
        }
        let mut snapshot = self.snapshot(&Conversation::default()).await?;
        snapshot.conversation = std::mem::take(conversation);
        let mut paging = self.paging.take().unwrap_or_default();
        let checkpoint = encode_snapshot(&mut snapshot, policy.pages_messages(), &mut paging);
        self.paging.put(paging);
        *conversation = snapshot.conversation;
        let checkpoint = checkpoint?;
        let settlement = match result {
            Ok(result) => Settlement::Complete(turn_output(
                result,
                conversation
                    .messages
                    .last()
                    .filter(|_| policy.pages_messages()),
            )?),
            Err(NanocodeError::TurnCancelled) => Settlement::Cancel,
            Err(NanocodeError::Response(error)) if error.retryable() => {
                Settlement::FailAttempt(error.to_string())
            }
            Err(error) => Settlement::Fail(error.to_string()),
        };
        let settled = policy
            .settle_encoded(operation.clone(), checkpoint, settlement)
            .await;
        if settled.is_err() {
            self.stopped.store(true, Ordering::SeqCst);
        }
        if let Some(turn) = self.steering.acquire().await.get_mut(&request.key) {
            turn.operation = None;
        }
        self.persistence_ready.notify_waiters();
        settled
    }
    pub(super) async fn durable_tool(
        &self,
        control: (&Cursor, &Cancellation),
        call: (&str, &str, &Value),
        handler: Option<&Handler>,
        events: &AgentEventPublisher,
        journal: (Vec<JournalWrite>, bool),
    ) -> Result<(ContentBlock, Option<JournalWrite>)> {
        #[derive(Serialize)]
        struct Call<'a> {
            id: &'a str,
            input: &'a Value,
            name: &'a str,
        }
        #[derive(Serialize)]
        struct Completed<'a> {
            result: &'a ContentBlock,
            tasks: Option<Value>,
            discovered: &'a HashSet<String>,
        }
        let (cursor, cancel) = control;
        let (id, name, input) = call;
        let (writes, defer) = journal;
        let index = cursor.index;
        let step = format!("tool-{index}-{id}");
        let effect = cursor.effect(self, &step);
        let admitted = if let Some(effect) = &effect {
            effect
                .begin_tool(
                    serde_json::value::to_raw_value(&Call { id, input, name })
                        .map_err(provider_error)?,
                    writes,
                )
                .await?
        } else {
            Step::Execute
        };
        let interrupted = matches!(&admitted, Step::Interrupted);
        if let Step::Replay(value) = admitted {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Receipt {
                result: ContentBlock,
                tasks: Option<Value>,
                discovered: HashSet<String>,
            }
            let mut receipt: Receipt = serde_json::from_value(value).map_err(recovery_error)?;
            let content = match &mut receipt.result {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } if tool_use_id == id => content,
                _ => {
                    return Err(recovery_error(
                        "tool receipt does not match the admitted call",
                    ));
                }
            };
            super::images::prepare_tool_images(
                content,
                crate::prompt::ImageResolution::of(&cursor.template.model),
                &self.images,
            )
            .await?;
            self.restore_tasks(receipt.tasks).map_err(recovery_error)?;
            self.discovered
                .update(|discovered| discovered.extend(receipt.discovered.iter().cloned()));
            return Ok((receipt.result, None));
        }
        let unknown = || {
            ContentBlock::tool_result_content(id, ToolResultContent::Text("Tool execution interrupted; outcome unknown. Do not assume it did not run or automatically repeat it.".into()), true)
        };
        let recovered = match handler {
            Some(handler) if interrupted => {
                self.recover_tool(id, name, input, handler, events, (cursor, cancel))
                    .await?
            }
            _ => None,
        };
        let interrupted = interrupted && recovered.is_none();
        let result = if let Some(recovered) = recovered {
            recovered
        } else if interrupted {
            ContentBlock::tool_result_content(id, ToolResultContent::Text("Tool execution interrupted by a host crash; outcome unknown. Do not assume it did not run or automatically repeat it.".into()), true)
        } else if cancel.is_cancelled() {
            unknown()
        } else if self.code_only && cursor.recovered_code_index == Some(index) && name == "exec" {
            ContentBlock::tool_result_content(id, ToolResultContent::Text(
                "Code Mode admission was lost during recovery; prior effects may have outcome unknown. No code was executed in this attempt. Reconcile those effects before using a fresh cell from a new model request.".into()
            ), true)
        } else if let Some(handler) = handler
            && matches!(name, "exec" | "wait")
        {
            self.call_tool(id, name, input, handler, events, (cursor, cancel, None))
                .await?
        } else if let Some(handler) = handler {
            tokio::select! {
                biased;
                result = self.call_tool(id, name, input, handler, events, (cursor, cancel, None)) => result?,
                () = cancel.cancelled() => unknown(),
            }
        } else if self.code_only
            && name != "exec"
            && name != "wait"
            && cursor
                .template
                .tools
                .iter()
                .any(|tool| matches!(tool, ClaudeToolSpec::Client(tool) if tool.name == name))
        {
            ContentBlock::tool_result_content(
                id,
                ToolResultContent::Text(format!(
                    "Direct tool {name} was retired during Code Mode recovery. Its prior outcome is unknown; no handler was invoked in this attempt. Reconcile effects before repeating through exec."
                )),
                true,
            )
        } else if self.code_only
            && self
                .nested_tools
                .as_ref()
                .is_some_and(|nested| nested(name))
        {
            let call = if name.contains('-') || name.starts_with(|c: char| c.is_ascii_digit()) {
                format!("use the `tools` entry listed as {name} in `ALL_TOOLS`")
            } else {
                format!("`await tools.{name}(...)`")
            };
            ContentBlock::tool_result_content(
                id,
                ToolResultContent::Text(format!(
                    "{name} is not a top-level tool; no handler was invoked. Only exec and wait are top-level. Call {name} from JavaScript inside exec through the global tools object: {call}."
                )),
                true,
            )
        } else {
            ContentBlock::tool_result_content(
                id,
                ToolResultContent::Text(
                    "Tool is not available in the admitted catalog or current host; no handler was invoked. Use an available tool."
                        .into(),
                ),
                true,
            )
        };
        let mut deferred = None;
        if let Some(effect) = &effect {
            let completion = effect.completion(&Completed {
                result: &result,
                tasks: self.task_snapshot()?,
                discovered: &self.discovered.get(),
            })?;
            if defer {
                deferred = Some(completion);
            } else {
                self.policy_writes(effect, vec![completion]).await?;
            }
        }
        if interrupted {
            self.emit(
                events,
                AgentEventKind::ToolResult,
                json!({
                    "call_id": id,
                    "tool": name,
                    "status": "unknown",
                    "result": "Tool execution interrupted by a host crash; outcome unknown. Do not assume it did not run or automatically repeat it.",
                    "outcome_unknown": true,
                    "duration_ns": 0,
                    "started_after_ns": null,
                }),
            );
        }
        Ok((result, deferred))
    }
    pub(super) async fn policy_writes(
        &self,
        effect: &Effect<'_>,
        writes: Vec<JournalWrite>,
    ) -> Result<()> {
        effect
            .policy
            .commit_writes(effect.operation.to_owned(), writes, None)
            .await
            .map(drop)
    }
}

const PAGED_VERSION: u32 = 4;

#[derive(Serialize)]
struct PagedConversation {
    messages: Sequence,
    code_calls: Sequence,
    #[serde(skip_serializing_if = "Option::is_none")]
    admitted_base: Option<EncodedPayload>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    admitted_tool_ids: Vec<Arc<str>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unadmitted_tool_ids: Vec<Arc<str>>,
}

#[derive(Serialize)]
struct PagedSnapshot<'a> {
    provider: &'a str,
    version: u32,
    #[serde(flatten)]
    conversation: PagedConversation,
    snapshot: &'a RawValue,
}

#[derive(Serialize)]
struct PagedCursor<'a> {
    version: u32,
    #[serde(flatten)]
    conversation: PagedConversation,
    pending: Sequence,
    template: EncodedPayload,
    cursor: &'a RawValue,
}

#[derive(Serialize)]
struct AdmittedBase<'a> {
    ids: &'a BTreeSet<Arc<str>>,
}

struct Detached {
    messages: Vec<HistoryMessage>,
    code_calls: Vec<history::CodeRound>,
    admitted_tool_ids: history::AdmittedIds,
}

impl Detached {
    fn take(conversation: &mut Conversation) -> Self {
        Self {
            messages: std::mem::take(&mut conversation.messages),
            code_calls: std::mem::take(&mut conversation.code_calls),
            admitted_tool_ids: std::mem::take(&mut conversation.admitted_tool_ids),
        }
    }

    fn restore(self, conversation: &mut Conversation) {
        conversation.messages = self.messages;
        conversation.code_calls = self.code_calls;
        conversation.admitted_tool_ids = self.admitted_tool_ids;
    }

    fn page(
        &self,
        records: &mut Vec<EncodedPayload>,
        paging: &mut Paging,
        settled: bool,
    ) -> Result<PagedConversation> {
        let unchanged = paging.messages.unchanged(&self.messages);
        let tools = &mut paging.tools;
        if tools.epoch != self.admitted_tool_ids.epoch()
            || tools.seen > self.admitted_tool_ids.len()
        {
            *tools = ToolIds::new(&self.admitted_tool_ids);
            for message in &paging.messages.items[..unchanged] {
                tools.list_message(message, &self.admitted_tool_ids);
            }
        }
        for message in &paging.messages.items[unchanged..] {
            tools.unlist_message(message, &self.admitted_tool_ids);
        }
        for message in &self.messages[unchanged..] {
            tools.list_message(message, &self.admitted_tool_ids);
        }
        tools.admit_new(&self.admitted_tool_ids);
        if settled {
            tools.settle(self.admitted_tool_ids.len());
        }
        let admitted_base = tools.base_record()?;
        if let Some(base) = &admitted_base {
            records.push(base.clone());
        }
        Ok(PagedConversation {
            messages: paging.messages.page(&self.messages, unchanged, records)?,
            code_calls: {
                let unchanged = paging.code_calls.unchanged(&self.code_calls);
                paging
                    .code_calls
                    .page(&self.code_calls, unchanged, records)?
            },
            admitted_base: admitted_base.as_ref().map(EncodedPayload::reference),
            admitted_tool_ids: tools.recent.iter().cloned().collect(),
            unadmitted_tool_ids: tools.unadmitted.iter().cloned().collect(),
        })
    }
}

#[derive(Default)]
struct ToolIds {
    epoch: u64,
    seen: usize,
    mark: usize,
    listed: HashMap<Arc<str>, u32>,
    recent: BTreeSet<Arc<str>>,
    base: BTreeSet<Arc<str>>,
    unadmitted: BTreeSet<Arc<str>>,
    base_record: Option<EncodedPayload>,
}

impl ToolIds {
    fn new(admitted: &history::AdmittedIds) -> Self {
        Self {
            epoch: admitted.epoch(),
            mark: admitted.len(),
            ..Self::default()
        }
    }

    fn tool_uses(message: &HistoryMessage) -> impl Iterator<Item = &str> {
        message.content.iter().filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
    }

    fn unlisted(&mut self, id: &str, admitted: &history::AdmittedIds) {
        match admitted.position(id) {
            Some(position) if position < self.mark => {
                self.base.insert(id.into());
                self.base_record = None;
            }
            Some(_) => {
                self.recent.insert(id.into());
            }
            None => {
                self.unadmitted.remove(id);
            }
        }
    }

    fn list_message(&mut self, message: &HistoryMessage, admitted: &history::AdmittedIds) {
        for id in Self::tool_uses(message) {
            let count = self.listed.entry(id.into()).or_insert(0);
            *count += 1;
            if *count > 1 {
                continue;
            }
            if admitted.contains(id) {
                self.recent.remove(id);
                if self.base.remove(id) {
                    self.base_record = None;
                }
            } else {
                self.unadmitted.insert(id.into());
            }
        }
    }

    fn unlist_message(&mut self, message: &HistoryMessage, admitted: &history::AdmittedIds) {
        for id in Self::tool_uses(message) {
            let Some(count) = self.listed.get_mut(id) else {
                continue;
            };
            *count -= 1;
            if *count == 0 {
                self.listed.remove(id);
                self.unlisted(id, admitted);
            }
        }
    }

    fn admit_new(&mut self, admitted: &history::AdmittedIds) {
        for id in &admitted.ordered()[self.seen..] {
            if self.listed.contains_key(id) {
                self.unadmitted.remove(id);
            } else {
                self.unlisted(id, admitted);
            }
        }
        self.seen = admitted.len();
    }

    fn settle(&mut self, mark: usize) {
        self.mark = mark;
        if !self.recent.is_empty() {
            self.base.append(&mut self.recent);
            self.base_record = None;
        }
    }

    fn base_record(&mut self) -> Result<Option<EncodedPayload>> {
        if self.base.is_empty() {
            return Ok(None);
        }
        if self.base_record.is_none() {
            self.base_record = Some(
                EncodedPayload::encode(&AdmittedBase { ids: &self.base })
                    .map_err(provider_error)?,
            );
        }
        Ok(self.base_record.clone())
    }
}

#[derive(Serialize)]
struct TemplatePrompt<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a Value>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    tools: &'a [ClaudeToolSpec],
}

pub(super) trait Paged: Clone {
    fn same(&self, other: &Self) -> bool;
    fn record(&self) -> serde_json::Result<EncodedPayload>;
}

impl Paged for HistoryMessage {
    fn same(&self, other: &Self) -> bool {
        self.ptr_eq(other)
    }

    fn record(&self) -> serde_json::Result<EncodedPayload> {
        Self::record(self)
    }
}

impl Paged for history::CodeRound {
    fn same(&self, other: &Self) -> bool {
        self.ptr_eq(other)
    }

    fn record(&self) -> serde_json::Result<EncodedPayload> {
        Self::record(self)
    }
}

pub(super) struct Pages<T> {
    items: Vec<T>,
    log: SequenceLog,
}

impl<T> Default for Pages<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            log: SequenceLog::default(),
        }
    }
}

impl<T: Paged> Pages<T> {
    fn unchanged(&self, items: &[T]) -> usize {
        self.items
            .iter()
            .zip(items)
            .take_while(|(cached, item)| cached.same(item))
            .count()
            .min(self.log.len())
    }

    fn page(
        &mut self,
        items: &[T],
        unchanged: usize,
        records: &mut Vec<EncodedPayload>,
    ) -> Result<Sequence> {
        self.log
            .sync(
                unchanged,
                items[unchanged..].iter().map(|item| {
                    item.record()
                        .map_err(nanocode_durability::Error::InvalidPayload)
                }),
            )
            .map_err(provider_error)?;
        self.items.truncate(unchanged);
        self.items.extend_from_slice(&items[unchanged..]);
        records.extend(self.log.frontier().cloned());
        Ok(self.log.sequence())
    }
}

#[derive(Default)]
pub(super) struct Paging {
    messages: Pages<HistoryMessage>,
    code_calls: Pages<history::CodeRound>,
    pending: Pages<HistoryMessage>,
    tools: ToolIds,
    template: Option<(Option<Value>, Vec<ClaudeToolSpec>, EncodedPayload)>,
}

fn template_record(prompt: &TemplatePrompt<'_>, paging: &mut Paging) -> Result<EncodedPayload> {
    if let Some((system, tools, record)) = &paging.template
        && system.as_ref() == prompt.system
        && tools.as_slice() == prompt.tools
    {
        return Ok(record.clone());
    }
    let record = EncodedPayload::encode(prompt).map_err(provider_error)?;
    paging.template = Some((
        prompt.system.cloned(),
        prompt.tools.to_vec(),
        record.clone(),
    ));
    Ok(record)
}

fn encode_snapshot(
    snapshot: &mut Snapshot,
    paged: bool,
    paging: &mut Paging,
) -> Result<EncodedState> {
    if !paged {
        return snapshot
            .conversation
            .messages
            .iter()
            .try_for_each(HistoryMessage::cache_json)
            .and_then(|()| serde_json::value::to_raw_value(&*snapshot))
            .map(|json| EncodedState {
                json,
                records: Vec::new(),
            })
            .map_err(provider_error);
    }
    let detached = Detached::take(&mut snapshot.conversation);
    let encoded = paged_snapshot(snapshot, &detached, paging);
    detached.restore(&mut snapshot.conversation);
    encoded
}

fn paged_snapshot(
    snapshot: &Snapshot,
    detached: &Detached,
    paging: &mut Paging,
) -> Result<EncodedState> {
    let mut records = Vec::new();
    let conversation = detached.page(&mut records, paging, true)?;
    let inner = serde_json::value::to_raw_value(snapshot).map_err(provider_error)?;
    let json = serde_json::value::to_raw_value(&PagedSnapshot {
        provider: &snapshot.provider,
        version: PAGED_VERSION,
        conversation,
        snapshot: &inner,
    })
    .map_err(provider_error)?;
    Ok(EncodedState { json, records })
}

fn paged_cursor(
    cursor: &Cursor,
    detached: &Detached,
    pending: &[HistoryMessage],
    prompt: &TemplatePrompt<'_>,
    paging: &mut Paging,
) -> Result<EncodedState> {
    let mut records = Vec::new();
    let conversation = detached.page(&mut records, paging, false)?;
    let unchanged = paging.pending.unchanged(pending);
    let pending = paging.pending.page(pending, unchanged, &mut records)?;
    let template = template_record(prompt, paging)?;
    let inner = serde_json::value::to_raw_value(cursor).map_err(provider_error)?;
    let json = serde_json::value::to_raw_value(&PagedCursor {
        version: PAGED_VERSION,
        conversation,
        pending,
        template: template.reference(),
        cursor: &inner,
    })
    .map_err(provider_error)?;
    records.push(template);
    Ok(EncodedState { json, records })
}

fn encode_cursor(cursor: &mut Cursor, paged: bool, paging: &mut Paging) -> Result<EncodedState> {
    if paged {
        let detached = Detached::take(&mut cursor.snapshot.conversation);
        let pending = std::mem::take(&mut cursor.pending);
        let system = cursor.template.system.take();
        let tools = std::mem::take(&mut cursor.template.tools);
        let prompt = TemplatePrompt {
            system: system.as_ref(),
            tools: &tools,
        };
        let encoded = paged_cursor(cursor, &detached, &pending, &prompt, paging);
        detached.restore(&mut cursor.snapshot.conversation);
        cursor.pending = pending;
        cursor.template.system = system;
        cursor.template.tools = tools;
        return encoded;
    }
    let shared = cursor
        .snapshot
        .conversation
        .messages
        .iter()
        .zip(&cursor.pending)
        .take_while(|(committed, pending)| committed.ptr_eq(pending))
        .count();
    let base = u32::try_from(shared).ok().filter(|base| *base > 0);
    let pending = std::mem::take(&mut cursor.pending);
    cursor.pending = pending[base.map_or(0, |_| shared)..].to_vec();
    cursor.pending_base = base;
    let encoded = cursor
        .snapshot
        .conversation
        .messages
        .iter()
        .chain(&cursor.pending)
        .try_for_each(HistoryMessage::cache_json)
        .and_then(|()| serde_json::value::to_raw_value(&*cursor));
    cursor.pending = pending;
    cursor.pending_base = None;
    encoded
        .map(|json| EncodedState {
            json,
            records: Vec::new(),
        })
        .map_err(provider_error)
}

fn turn_output(result: &TurnResult, last: Option<&HistoryMessage>) -> Result<Box<RawValue>> {
    let final_message = result.final_message();
    let record = last
        .filter(|message| {
            message.role == Role::Assistant
                && !final_message.is_empty()
                && text_of(message) == final_message
        })
        .map(HistoryMessage::record)
        .transpose()
        .map_err(provider_error)?;
    match record {
        Some(record) => serde_json::value::to_raw_value(
            &json!({"final_record": record.key(), "usage": result.usage()}),
        ),
        None => serde_json::value::to_raw_value(
            &json!({"final_message": final_message, "usage": result.usage()}),
        ),
    }
    .map_err(provider_error)
}

fn text_of(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub(super) fn replay(operation: String, output: Value) -> Result<TurnResult> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Output {
        final_message: String,
        usage: Option<TurnUsage>,
    }
    let output: Output = serde_json::from_value(output).map_err(recovery_error)?;
    Ok(TurnResult::from_backend(
        Some(operation),
        output.final_message,
        output.usage,
        None,
    ))
}
pub(super) fn candidate_id(kind: &str) -> String {
    format!("claude-{kind}-{}", nanocode_platform::uuid_v4())
}

pub(super) fn recovery_error(error: impl std::fmt::Display) -> NanocodeError {
    NanocodeError::execution_policy_with_disposition(
        "Claude recovery",
        nanocode_agent_core::ExecutionPolicyDisposition::Reopen,
        provider_error(error),
    )
}

pub fn rewind_checkpoint(
    source_session_id: &str,
    previous: Option<Value>,
    latest: Value,
) -> Result<Value> {
    let latest = Snapshot::decode(latest)?;

    let source_lineage = latest
        .lineage
        .clone()
        .unwrap_or_else(|| Lineage::root(source_session_id));
    if latest.conversation.pending_continuation {
        return Err(unsupported(
            "conversation rewind refuses a pending tool/provider continuation",
        ));
    }
    let mut selected = match previous {
        Some(value) => Snapshot::decode(value)?,
        None => Snapshot {
            model: latest.model.clone(),
            workspace: latest.workspace.clone(),
            effort: latest.effort,
            thinking_disabled: latest.thinking_disabled,
            fast_mode: latest.fast_mode,
            ..Snapshot::default()
        },
    };
    if selected.conversation.pending_continuation {
        return Err(unsupported(
            "selected checkpoint has a pending tool/provider continuation",
        ));
    }
    selected
        .conversation
        .admitted_tool_ids
        .extend(latest.conversation.admitted_tool_ids);
    for notice in latest.conversation.recovery_notices {
        if !selected.conversation.recovery_notices.contains(&notice) {
            selected.conversation.recovery_notices.push(notice);
        }
    }
    let notice = "This conversation was explicitly rewound into a new session. External effects from discarded turns may still exist; reconcile their current state before repeating any action. Historical tool calls must not be replayed.".to_owned();
    if !selected.conversation.recovery_notices.contains(&notice) {
        selected.conversation.recovery_notices.push(notice);
    }
    selected.conversation.lifecycle_started = false;
    selected.conversation.container = None;
    selected.conversation.previous_message_id = None;

    selected.lineage = Some(Lineage::child_of(
        &source_lineage,
        source_session_id,
        Origin::Branch,
    ));
    selected.conversation_id = None;
    serde_json::to_value(selected).map_err(provider_error)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaudeCheckpointView {
    pub model: Option<ModelSelection>,

    pub workspace: Option<String>,

    pub lineage: Option<Lineage>,

    pub transcript: Vec<nanocode_agent_core::session::TranscriptItem>,
}

pub fn decode_checkpoint(checkpoint: Value) -> Result<ClaudeCheckpointView> {
    let snapshot: Snapshot = serde_json::from_value::<Snapshot>(checkpoint)
        .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?
        .validated()?;
    snapshot.view()
}

pub fn session_checkpoint(
    session_id: &str,
    lineage: Lineage,
    checkpoint: Value,
) -> Result<SessionCheckpoint> {
    let mut snapshot = Snapshot::decode(checkpoint)?;
    let model = snapshot
        .model
        .clone()
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| {
            NanocodeError::InvalidCheckpoint("checkpoint records no Claude model".into())
        })?;
    let conversation_id = snapshot
        .conversation_id
        .clone()
        .unwrap_or_else(|| session_id.to_owned());
    snapshot.lineage = Some(lineage.clone());
    let policy = NativePolicy {
        context_window_tokens: default_context_window_tokens(&model),
        max_tokens: None,
        effort: snapshot.effort,
        adaptive_thinking: snapshot.effort.is_some(),
        thinking_disabled: snapshot.thinking_disabled,
        automatic_cache: false,
        cache_one_hour: false,
        keep_thinking: false,
        fast_mode: snapshot.fast_mode,
        message_diagnostics: false,
        auto_compact_window_tokens: None,
        model,
    };
    ClaudeBoundary {
        snapshot: Arc::new(snapshot),
        policy,
        session_id: session_id.to_owned(),
        lineage,
        conversation_id,
    }
    .checkpoint()
}

pub fn decode_session_checkpoint(checkpoint: &SessionCheckpoint) -> Result<ClaudeCheckpointView> {
    checkpoint.validate()?;
    checkpoint.require_family(HarnessFamily::Claude)?;
    let stored: NativeChildState = serde_json::from_value(checkpoint.payload().clone())
        .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))?;
    let mut view = stored.snapshot.validated()?.view()?;
    view.model = Some(checkpoint.model());
    view.lineage = Some(checkpoint.lineage().clone());
    Ok(view)
}

impl Snapshot {
    fn view(&self) -> Result<ClaudeCheckpointView> {
        let model = self
            .model
            .as_deref()
            .map(|model| {
                (!model.trim().is_empty())
                    .then(|| super::claude_selection(model))
                    .ok_or_else(|| {
                        NanocodeError::InvalidCheckpoint(
                            "checkpoint model is not a Claude model".into(),
                        )
                    })
            })
            .transpose()?;
        Ok(ClaudeCheckpointView {
            model,
            workspace: self
                .workspace
                .clone()
                .filter(|workspace| !workspace.is_empty()),
            lineage: self.lineage.clone(),
            transcript: self.transcript(),
        })
    }
    fn transcript(&self) -> Vec<nanocode_agent_core::session::TranscriptItem> {
        use nanocode_agent_core::session::TranscriptItem;
        let mut items = Vec::new();
        if !self.conversation.summary.is_empty() {
            items.push(TranscriptItem::Assistant(format!(
                "Retained conversation summary:\n{}",
                self.conversation.summary
            )));
        }
        for message in &self.conversation.messages {
            for block in &message.content {
                match block {
                    ContentBlock::Text { text, .. } => items.push(match message.role {
                        Role::Assistant => TranscriptItem::Assistant(text.clone()),
                        Role::User => TranscriptItem::User(text.clone()),
                    }),
                    ContentBlock::Thinking { thinking, .. } if !thinking.trim().is_empty() => {
                        items.push(TranscriptItem::Reasoning(thinking.clone()));
                    }
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    }
                    | ContentBlock::ServerToolUse {
                        id, name, input, ..
                    } => {
                        items.push(TranscriptItem::Tool {
                            call_id: id.clone(),
                            name: name.clone(),
                            arguments: input.to_string(),
                            parent_call_id: None,
                        });
                    }
                    ContentBlock::McpToolUse {
                        id,
                        name,
                        server_name,
                        input,
                        ..
                    } => items.push(TranscriptItem::Tool {
                        call_id: id.clone(),
                        name: format!("mcp__{server_name}__{name}"),
                        arguments: input.to_string(),
                        parent_call_id: None,
                    }),
                    _ => {}
                }
            }
        }
        items
    }
}
