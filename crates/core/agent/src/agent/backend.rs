use super::*;

const SIDE_QUESTION_REMINDER: &str = "<system-reminder>This is a side question from the user. You must answer this question directly in a single response.
IMPORTANT CONTEXT:
- You are a separate, lightweight agent spawned to answer this one question
- The main agent is NOT interrupted - it continues working independently in the background
- You share the conversation context but are a completely separate instance
- Do NOT reference being interrupted or what you were \"previously doing\" - that framing is incorrect
CRITICAL CONSTRAINTS:
- You have NO tools available - you cannot read files, run commands, search, or take any actions
- Do NOT write tool calls or tool output as text (for example invoke or function_calls XML blocks) - nothing you write here is executed; if answering would need reading files, running commands, or searching, say that can't be checked from a side question and suggest asking in the main conversation
- This is a one-off response - there will be no follow-up turns
- You can ONLY provide information based on what you already know from the conversation context
- NEVER say things like \"Let me try...\", \"I'll now...\", \"Let me check...\", or promise to take any action
- If you don't know the answer, say so - do not offer to look it up or investigate
Simply answer the question with the information you have.</system-reminder>";

#[must_use]
pub fn side_question_prompt(question: &str) -> String {
    format!("{SIDE_QUESTION_REMINDER}\n\n{question}")
}

#[derive(Default)]
pub struct TranscriptBuilder {
    messages: Vec<(nanocode_types::PromptMessageRole, String)>,
}

impl TranscriptBuilder {
    pub fn user(&mut self, text: &str) {
        self.push(nanocode_types::PromptMessageRole::User, text);
    }

    pub fn assistant(&mut self, text: &str) {
        self.push(nanocode_types::PromptMessageRole::Assistant, text);
    }

    pub fn tool_call(&mut self, name: &str, input: &str) {
        self.assistant(&format!("[Tool call: {name}]\n{input}"));
    }

    pub fn tool_result(&mut self, name: &str, output: &str) {
        self.user(&format!("[Tool result: {name}]\n{output}"));
    }

    fn push(&mut self, role: nanocode_types::PromptMessageRole, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        match self.messages.last_mut() {
            Some((last, content)) if *last == role => {
                content.push_str("\n\n");
                content.push_str(text);
            }
            _ => self.messages.push((role, text.to_owned())),
        }
    }

    #[must_use]
    pub fn finish(mut self) -> Vec<nanocode_types::PromptMessage> {
        use nanocode_types::{PromptMessage, PromptMessageRole};
        if self.messages.is_empty() {
            return Vec::new();
        }
        if self
            .messages
            .first()
            .is_some_and(|(role, _)| *role == PromptMessageRole::Assistant)
        {
            self.messages.insert(
                0,
                (
                    PromptMessageRole::User,
                    "[The earlier part of this conversation is not available.]".to_owned(),
                ),
            );
        }
        if self
            .messages
            .last()
            .is_some_and(|(role, _)| *role == PromptMessageRole::User)
        {
            self.messages.push((
                PromptMessageRole::Assistant,
                "[No assistant reply was recorded after the message above.]".to_owned(),
            ));
        }
        self.messages
            .into_iter()
            .map(|(role, content)| match role {
                PromptMessageRole::User => PromptMessage::user(content),
                PromptMessageRole::Assistant => PromptMessage::assistant(content),
            })
            .collect()
    }
}

pub trait BuilderBackend {
    type Builder;

    fn into_builder(self) -> Self::Builder;
}

#[cfg(not(target_family = "wasm"))]
pub type BackendFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[cfg(target_family = "wasm")]
pub type BackendFuture<T> = Pin<Box<dyn Future<Output = T> + 'static>>;

#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BackendTurnKey(pub u64);

#[doc(hidden)]
#[derive(Clone)]
pub struct BackendPrompt {
    pub key: BackendTurnKey,

    pub prompt: Prompt,

    pub request_id: Option<String>,

    pub cancel_on_admission: bool,

    pub events: nanocode_types::events::AgentEventPublisher,
}

#[doc(hidden)]
pub struct BackendTurn {
    pub request_id: Option<String>,

    pub result: BackendFuture<Result<TurnResult>>,
}

#[doc(hidden)]
pub enum BackendPromptRoute {
    Started(BackendTurn),

    Steered,
}

pub trait ChildJournalStore: Send + Sync + 'static {
    fn pending_turn(
        &self,
        _session_id: String,
    ) -> BackendFuture<std::io::Result<Option<(String, serde_json::Value)>>> {
        Box::pin(async { Ok(None) })
    }

    fn cancelled_turns(
        &self,
        _session_id: String,
        _operation_ids: Vec<String>,
    ) -> BackendFuture<std::io::Result<Vec<String>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn load(&self) -> BackendFuture<std::io::Result<Option<String>>>;

    fn save(&self, payload: String, records: Vec<Arc<str>>) -> BackendFuture<std::io::Result<()>>;

    fn load_record(&self, key: String) -> BackendFuture<std::io::Result<String>>;

    fn load_records(&self, keys: Vec<String>) -> BackendFuture<std::io::Result<Vec<String>>> {
        let loads = keys
            .into_iter()
            .map(|key| self.load_record(key))
            .collect::<Vec<_>>();
        Box::pin(async move {
            let mut records = Vec::with_capacity(loads.len());
            for load in loads {
                records.push(load.await?);
            }
            Ok(records)
        })
    }

    fn record_child(&self, _checkpoint: SessionCheckpoint) -> BackendFuture<std::io::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn records_children(&self) -> bool {
        true
    }
}

#[derive(Clone)]
pub struct ChildJournal {
    store: Arc<dyn ChildJournalStore>,
    owner: Arc<std::sync::OnceLock<Arc<str>>>,
}

impl ChildJournal {
    pub fn new(store: Arc<dyn ChildJournalStore>) -> Self {
        Self {
            store,
            owner: Arc::new(std::sync::OnceLock::new()),
        }
    }

    pub fn claim(&self, session_id: &str) -> bool {
        &**self.owner.get_or_init(|| Arc::from(session_id)) == session_id
    }

    pub fn store(&self) -> Arc<dyn ChildJournalStore> {
        Arc::clone(&self.store)
    }
}

impl std::fmt::Debug for ChildJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildJournal").finish_non_exhaustive()
    }
}

pub trait AgentFactory: Send + Sync + 'static {
    fn spawn(
        &self,
        parent: AgentHandle,
        options: SpawnOptions,
        host_context: Option<Arc<str>>,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>>;

    fn ensure_available(&self, parent: AgentHandle) -> BackendFuture<Result<()>> {
        let settings = self.settings(parent);
        Box::pin(async move { settings.await.map(|_| ()) })
    }

    fn spawn_many(
        &self,
        parent: AgentHandle,
        count: usize,
        observer: Arc<dyn Fn(&str) + Send + Sync>,
        host_context: Option<Arc<str>>,
    ) -> BackendFuture<Result<Vec<(Agent, AgentEvents)>>> {
        let pending = (0..count)
            .map(|_| self.spawn(parent.clone(), SpawnOptions::new(), host_context.clone()))
            .collect::<Vec<_>>();
        Box::pin(async move {
            let mut children: Vec<(Agent, AgentEvents)> = Vec::with_capacity(count);
            for child in pending {
                match child.await {
                    Ok(child) => children.push(child),
                    Err(error) => {
                        for (child, _) in &children {
                            let _ = child.shutdown().await;
                        }
                        return Err(error);
                    }
                }
            }

            for (child, _) in &children {
                observer(child.session_id());
            }
            Ok(children)
        })
    }

    fn settings(
        &self,
        parent: AgentHandle,
    ) -> BackendFuture<Result<(crate::ModelSelection, Thinking)>> {
        Box::pin(async move {
            let model = parent.harness_model();
            Ok((model.clone(), model.default_thinking()))
        })
    }

    fn reasoning_mode(&self, _parent: AgentHandle) -> BackendFuture<Result<crate::ReasoningMode>> {
        Box::pin(async { Ok(crate::ReasoningMode::Standard) })
    }

    fn fork(
        &self,
        _parent: AgentHandle,
        _request: ForkRequest,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        unsupported("fork")
    }

    fn restore(
        &self,
        _parent: AgentHandle,
        _checkpoint: SessionCheckpoint,
        _host_context: Option<Arc<str>>,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        unsupported("restore")
    }
}

pub struct CrossHarnessOnly;

impl AgentFactory for CrossHarnessOnly {
    fn spawn(
        &self,
        _parent: AgentHandle,
        _options: SpawnOptions,
        _host_context: Option<Arc<str>>,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        unsupported("same_harness_spawn")
    }
}

fn unsupported<T: 'static>(capability: &'static str) -> BackendFuture<Result<T>> {
    Box::pin(async move { Err(NanocodeError::UnsupportedCapability { capability }) })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SteerReceiptStatus {
    Accepted,
    Absent,
    Different,
    Withdrawn,
    Unknown,
}

#[doc(hidden)]
pub trait LifecycleBackend: Send + Sync + 'static {
    fn decode_durable_prompt(&self, input: serde_json::Value) -> Result<Prompt> {
        serde_json::from_value(input)
            .map_err(|error| NanocodeError::InvalidCheckpoint(error.to_string()))
    }

    fn harness_family(&self) -> crate::HarnessFamily;

    fn capabilities(&self) -> Capabilities;

    fn persistence(&self) -> Option<Persistence> {
        None
    }

    fn submit(&self, prompt: BackendPrompt) -> BackendFuture<Result<BackendTurn>>;

    fn route(&self, prompt: BackendPrompt) -> BackendFuture<Result<BackendPromptRoute>>;

    fn steer(&self, key: BackendTurnKey, prompt: Prompt) -> BackendFuture<Result<()>>;

    fn steer_with_id(
        &self,
        _key: BackendTurnKey,
        _id: String,
        _prompt: Prompt,
    ) -> BackendFuture<Result<()>> {
        unsupported("identified_steering")
    }

    fn withdraw_steer(&self, _key: BackendTurnKey, _id: String) -> BackendFuture<Result<bool>> {
        unsupported("identified_steering")
    }

    fn steer_receipt(
        &self,
        _id: String,
        _prompt: Prompt,
    ) -> BackendFuture<Result<SteerReceiptStatus>> {
        Box::pin(async { Ok(SteerReceiptStatus::Unknown) })
    }

    fn cancel(&self, key: BackendTurnKey) -> BackendFuture<Result<()>>;

    fn cancel_active(&self) -> BackendFuture<Result<()>> {
        unsupported("cancel_active_turn")
    }

    fn set_harness_model(&self, model: crate::ModelSelection) -> BackendFuture<Result<()>>;

    fn set_thinking(&self, thinking: Thinking) -> BackendFuture<Result<()>>;

    fn set_fast_mode(&self, enabled: bool) -> BackendFuture<Result<()>>;

    /// Sets the context size, in tokens, at which the agent compacts its history by itself, from
    /// the next turn on; `None` restores the backend's own default.
    fn set_auto_compact_token_limit(&self, _limit: Option<u64>) -> BackendFuture<Result<()>> {
        unsupported("auto_compact_token_limit")
    }

    fn set_service_tier(&self, service_tier: ServiceTier) -> BackendFuture<Result<()>> {
        match service_tier {
            ServiceTier::Standard => self.set_fast_mode(false),
            ServiceTier::Priority | ServiceTier::Fast => self.set_fast_mode(true),
            ServiceTier::Ultrafast => unsupported("ultrafast_service_tier"),
        }
    }

    fn compact(&self) -> BackendFuture<Result<()>>;

    fn preconnect(&self) -> BackendFuture<Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn side_question(&self, _question: String) -> BackendFuture<Result<SideAnswer>> {
        unsupported("side_question")
    }

    fn transcript(&self) -> BackendFuture<Result<Vec<nanocode_types::PromptMessage>>> {
        unsupported("transcript")
    }

    fn append_developer_message(&self, text: String) -> BackendFuture<Result<AgentSessionContext>>;

    fn append_developer_notice(
        &self,
        text: String,
        _prefix: Option<String>,
    ) -> BackendFuture<Result<bool>> {
        let appended = self.append_developer_message(text);
        Box::pin(async move { appended.await.map(|_| true) })
    }

    fn context(&self) -> BackendFuture<Result<AgentSessionContext>>;

    fn checkpoint(&self) -> BackendFuture<Result<SessionCheckpoint>>;

    fn persist_initial(&self) -> BackendFuture<Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn discard_initial(&self) -> BackendFuture<Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn spawn(&self, options: SpawnOptions) -> BackendFuture<Result<(Agent, AgentEvents)>>;

    fn fork(&self, request: ForkRequest) -> BackendFuture<Result<(Agent, AgentEvents)>>;

    fn flush(&self) -> BackendFuture<Result<()>>;

    fn disconnect(&self) -> BackendFuture<Result<()>> {
        self.shutdown()
    }

    fn shutdown(&self) -> BackendFuture<Result<()>>;

    fn is_stopped(&self) -> bool {
        false
    }

    fn owns_session(&self) -> BackendFuture<Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn reopen(
        &self,
        _events: nanocode_types::events::AgentEventPublisher,
    ) -> BackendFuture<Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[doc(hidden)]
pub struct BackendRuntime {
    agent_id: Arc<str>,
    session_id: Arc<str>,
    lineage: Lineage,
    driver_turn_ids: bool,
    events: nanocode_types::events::AgentEventPublisher,
}

impl BackendRuntime {
    #[must_use]
    pub fn new(session_id: impl Into<Arc<str>>) -> (Self, AgentEvents) {
        let session_id = session_id.into();
        Self::with_agent_id(Arc::clone(&session_id), session_id)
    }

    #[must_use]
    pub fn with_agent_id(
        agent_id: impl Into<Arc<str>>,
        session_id: impl Into<Arc<str>>,
    ) -> (Self, AgentEvents) {
        let agent_id = agent_id.into();
        let session_id = session_id.into();
        let (events, stream) =
            nanocode_types::events::AgentEventPublisher::channel(session_id.to_string());
        (
            Self {
                lineage: Lineage::root(session_id.as_ref()),
                agent_id,
                session_id,
                driver_turn_ids: false,
                events,
            },
            stream,
        )
    }

    #[must_use]
    pub fn with_lineage(mut self, lineage: Lineage) -> Self {
        self.lineage = lineage;
        self
    }

    #[must_use]
    pub const fn with_driver_turn_ids(mut self) -> Self {
        self.driver_turn_ids = true;
        self
    }

    #[must_use]
    pub fn events(&self) -> nanocode_types::events::AgentEventPublisher {
        self.events.clone()
    }

    #[must_use]
    pub fn bind<B>(self, backend: B) -> Agent
    where
        B: LifecycleBackend,
    {
        let session = SessionInfo {
            session_id: self.session_id.to_string(),
            family: backend.harness_family(),
            lineage: self.lineage,
        };
        Agent {
            backend: Arc::new(backend),
            events: self.events,
            next_turn: Arc::new(AtomicU64::new(1)),
            agent_id: self.agent_id,
            session: Arc::new(session),
            driver_turn_ids: self.driver_turn_ids,
            seed: Arc::default(),
            interrupted: Arc::default(),
            requests: Arc::default(),
            ownership: None,
        }
    }
}
