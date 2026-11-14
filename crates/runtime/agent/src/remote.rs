use std::sync::{Arc, Weak};

use nanocode_agent_core::{
    Agent, AgentEvents, AgentHandle, AgentSessionContext, Capabilities, ForkRequest, HarnessFamily,
    Lineage, ModelSelection, NanocodeError, Origin, Result, SessionCheckpoint, SpawnOptions,
    Thinking, TurnResult,
    backend::{
        AgentFactory, BackendFuture, BackendPrompt, BackendPromptRoute, BackendRuntime,
        BackendTurn, BackendTurnKey, LifecycleBackend,
    },
    events::{AgentEventKind, AgentEventPublisher},
    input::Prompt,
};
use nanocode_sync::{Atom, AtomMap};
use nanocode_tool_contracts::{Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput};
use serde_json::{Value, json, value::RawValue};

const REMOTE_MARKER: &str = "remote_child";

pub struct ChildSpec {
    pub host_context: Option<Arc<str>>,
    pub lineage: Lineage,
    pub model: ModelSelection,
    pub restored: bool,
    pub session_id: Arc<str>,
    pub thinking: Thinking,
    pub tools: Vec<ToolDefinition>,
}

pub struct ChildToolCall<'a> {
    pub call_id: &'a str,
    pub host_context: Option<&'a str>,
    pub input: ToolInput,
    pub instruction_revision: Option<u64>,
    pub name: &'a str,
    pub output_budget: usize,
    pub turn_id: Option<&'a str>,
}

pub trait ChildHost: Send + Sync + 'static {
    fn cancel(&self, session_id: Arc<str>, request_id: Option<String>)
    -> BackendFuture<Result<()>>;

    fn close(&self, session_id: Arc<str>) -> BackendFuture<Result<()>>;

    fn open(&self, child: ChildSpec) -> BackendFuture<Result<()>>;

    fn pending(&self, session_id: Arc<str>) -> BackendFuture<Result<Option<(String, Value)>>>;

    fn run(
        &self,
        session_id: Arc<str>,
        request_id: String,
        prompt: Prompt,
    ) -> BackendFuture<Result<TurnResult>>;

    fn steer(
        &self,
        session_id: Arc<str>,
        request_id: String,
        id: Option<String>,
        prompt: Prompt,
    ) -> BackendFuture<Result<()>>;
}

#[derive(Clone)]
struct Child {
    events: AgentEventPublisher,
    host_context: Option<Arc<str>>,
    lineage: Lineage,
    model: ModelSelection,
    thinking: Thinking,
    tools: Arc<Vec<Arc<dyn Tool>>>,
}

pub struct RemoteChildren {
    children: AtomMap<Arc<str>, Child>,
    host: Arc<dyn ChildHost>,
    registry: Weak<nanocode_subagents::Registry>,
    restored: AtomMap<Arc<str>, ()>,
    spawn_factory: Atom<Option<Arc<dyn AgentFactory>>>,
}

impl RemoteChildren {
    pub(crate) fn new(
        host: Arc<dyn ChildHost>,
        registry: &Arc<nanocode_subagents::Registry>,
    ) -> Arc<Self> {
        let children = Arc::new(Self {
            children: AtomMap::new(),
            host,
            registry: Arc::downgrade(registry),
            restored: AtomMap::new(),
            spawn_factory: Atom::new(None),
        });
        let remote = Arc::downgrade(&children);
        registry.set_pending_turn_resolver(Arc::new(move |session_id: &str| {
            let remote = remote.upgrade()?;
            remote.restored.remove_if(session_id, |()| true)?;
            let session_id: Arc<str> = session_id.into();
            Some(Box::pin(async move {
                remote
                    .host
                    .pending(session_id)
                    .await
                    .map_err(std::io::Error::other)
            }))
        }));
        children
    }

    pub(crate) fn set_spawn_factory(&self, factory: Arc<dyn AgentFactory>) {
        self.spawn_factory.set(Some(factory));
    }

    #[must_use]
    pub fn is_remote(checkpoint: &SessionCheckpoint) -> bool {
        checkpoint
            .payload()
            .decode::<Value>()
            .is_ok_and(|payload| payload.get(REMOTE_MARKER) == Some(&Value::Bool(true)))
    }

    #[must_use]
    pub fn contains(&self, session_id: &str) -> bool {
        self.children.contains_key(session_id)
    }

    pub fn publish(&self, session_id: &str, kind: AgentEventKind, payload: Arc<RawValue>) -> bool {
        self.children
            .get(session_id)
            .is_some_and(|child| child.events.publish_next(kind, payload).is_ok())
    }

    pub async fn call_tool(&self, session_id: &str, call: ChildToolCall<'_>) -> Result<ToolOutput> {
        let child = self.children.get(session_id).ok_or_else(|| {
            NanocodeError::InvalidRequest(format!("no remote child {session_id}"))
        })?;
        let tool = child
            .tools
            .iter()
            .find(|tool| tool.definition().name() == call.name)
            .cloned()
            .ok_or_else(|| {
                NanocodeError::InvalidRequest(format!(
                    "remote child {session_id} has no tool {}",
                    call.name
                ))
            })?;
        let model = child.model.as_str().to_owned();
        let host_context = call.host_context.or(child.host_context.as_deref());
        let context = ToolContext::new(&model, session_id, call.call_id, &[], call.output_budget)
            .with_turn_id(call.turn_id)
            .with_instruction_revision(call.instruction_revision)
            .with_host_context(host_context);
        Ok(match tool.execute(call.input, context).await {
            Ok(output) => output,
            Err(error) => ToolOutput::error(error.to_string()),
        })
    }

    fn parent_lineage(&self, parent: &AgentHandle) -> Lineage {
        self.children
            .get(parent.session_id())
            .map_or_else(|| Lineage::root(parent.session_id()), |child| child.lineage)
    }

    async fn agent(self: Arc<Self>, spec: ChildSpec) -> Result<(Agent, AgentEvents)> {
        let session_id = Arc::clone(&spec.session_id);
        let mut handle = AgentHandle::new(
            Arc::clone(&session_id),
            spec.model.clone(),
            Arc::new(ChildFactory(Arc::clone(&self))) as Arc<dyn AgentFactory>,
        )
        .with_root_session_id(spec.lineage.root_session_id.as_str());
        if let Some(factory) = Option::clone(&self.spawn_factory.get()) {
            handle = handle.with_spawn_factory(factory);
        }

        let registry = self.registry.upgrade().ok_or(NanocodeError::TurnStopped)?;
        let Ok(SharedTools(tools)) = if spec.model.family() == HarnessFamily::Claude {
            nanocode_subagents::install_claude_tools(SharedTools(Vec::new()), handle, registry)
        } else {
            nanocode_subagents::install_tools(SharedTools(Vec::new()), handle, registry)
        };
        let spec = ChildSpec {
            tools: tools.iter().map(|tool| tool.definition()).collect(),
            ..spec
        };

        let (runtime, events) = BackendRuntime::new(Arc::clone(&session_id));
        let runtime = runtime.with_lineage(spec.lineage.clone());
        let child = Child {
            events: runtime.events(),
            host_context: spec.host_context.clone(),
            lineage: spec.lineage.clone(),
            model: spec.model.clone(),
            thinking: spec.thinking,
            tools: Arc::new(tools),
        };
        self.children.insert(Arc::clone(&session_id), child.clone());
        if spec.restored {
            self.restored.insert(Arc::clone(&session_id), ());
        }
        if let Err(error) = self.host.open(spec).await {
            self.children.remove_if(&session_id, |_| true);
            self.restored.remove_if(&session_id, |()| true);
            return Err(error);
        }

        let backend = RemoteBackend {
            child,
            children: Arc::clone(&self),
            requests: Arc::new(AtomMap::new()),
            session_id,
            stopped: Atom::new(false),
        };
        Ok((runtime.bind(backend), events))
    }

    pub(crate) async fn spawn(
        self: Arc<Self>,
        parent: AgentHandle,
        options: SpawnOptions,
        host_context: Option<Arc<str>>,
    ) -> Result<(Agent, AgentEvents)> {
        let (model, thinking) = parent.settings().await?;
        let options = options.resolve(model, thinking)?;
        let model = options.selected_harness_model().ok_or_else(|| {
            NanocodeError::InvalidRequest("a remote child requires a resolved model".into())
        })?;
        let thinking = options.selected_thinking().ok_or_else(|| {
            NanocodeError::InvalidRequest("a remote child requires a resolved effort".into())
        })?;
        let session_id: Arc<str> = options
            .session_id
            .clone()
            .unwrap_or_else(|| nanocode_platform::uuid_v7().to_string())
            .into();
        let lineage = Lineage::child_of(
            &self.parent_lineage(&parent),
            parent.session_id(),
            Origin::Subagent,
        );

        self.agent(ChildSpec {
            host_context,
            lineage,
            model,
            restored: false,
            session_id,
            thinking,
            tools: Vec::new(),
        })
        .await
    }

    pub(crate) async fn restore(
        self: Arc<Self>,
        checkpoint: SessionCheckpoint,
        host_context: Option<Arc<str>>,
    ) -> Result<(Agent, AgentEvents)> {
        self.agent(ChildSpec {
            host_context,
            lineage: checkpoint.lineage().clone(),
            model: checkpoint.model(),
            restored: true,
            session_id: checkpoint.session_id().into(),
            thinking: checkpoint.thinking(),
            tools: Vec::new(),
        })
        .await
    }
}

struct SharedTools(Vec<Arc<dyn Tool>>);

impl nanocode_tool_contracts::ToolCollection for SharedTools {
    type Error = std::convert::Infallible;

    fn add_shared_tool(mut self, tool: Arc<dyn Tool>) -> std::result::Result<Self, Self::Error> {
        self.0.push(tool);
        Ok(self)
    }
}

struct ChildFactory(Arc<RemoteChildren>);

impl AgentFactory for ChildFactory {
    fn spawn(
        &self,
        parent: AgentHandle,
        options: SpawnOptions,
        host_context: Option<Arc<str>>,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        Box::pin(Arc::clone(&self.0).spawn(parent, options, host_context))
    }

    fn settings(&self, parent: AgentHandle) -> BackendFuture<Result<(ModelSelection, Thinking)>> {
        let settings = self
            .0
            .children
            .get(parent.session_id())
            .map(|child| (child.model, child.thinking))
            .ok_or_else(|| {
                NanocodeError::InvalidRequest(format!("no remote child {}", parent.session_id()))
            });
        Box::pin(async move { settings })
    }

    fn restore(
        &self,
        _parent: AgentHandle,
        checkpoint: SessionCheckpoint,
        host_context: Option<Arc<str>>,
    ) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        Box::pin(Arc::clone(&self.0).restore(checkpoint, host_context))
    }
}

struct RemoteBackend {
    child: Child,
    children: Arc<RemoteChildren>,
    requests: Arc<AtomMap<u64, String>>,
    session_id: Arc<str>,
    stopped: Atom<bool>,
}

fn unsupported<T: 'static>(capability: &'static str) -> BackendFuture<Result<T>> {
    Box::pin(async move { Err(NanocodeError::UnsupportedCapability { capability }) })
}

impl RemoteBackend {
    fn start(&self, prompt: BackendPrompt) -> BackendTurn {
        let key = prompt.key.0;
        let request_id = prompt
            .request_id
            .clone()
            .unwrap_or_else(|| nanocode_platform::uuid_v7().to_string());
        self.requests.insert(key, request_id.clone());
        let run = self.children.host.run(
            Arc::clone(&self.session_id),
            request_id.clone(),
            prompt.prompt,
        );
        let requests = Arc::clone(&self.requests);
        BackendTurn {
            request_id: Some(request_id),
            result: Box::pin(async move {
                let result = run.await;
                requests.remove_if(&key, |_| true);
                result
            }),
        }
    }

    fn steer_turn(
        &self,
        key: BackendTurnKey,
        id: Option<String>,
        prompt: Prompt,
    ) -> BackendFuture<Result<()>> {
        let Some(request_id) = self.requests.get(&key.0) else {
            return Box::pin(async { Err(NanocodeError::TurnNotSteerable) });
        };
        self.children
            .host
            .steer(Arc::clone(&self.session_id), request_id, id, prompt)
    }
}

impl LifecycleBackend for RemoteBackend {
    fn harness_family(&self) -> HarnessFamily {
        self.child.model.family()
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::NONE;
        capabilities.checkpoint = true;
        capabilities.identified_steering = true;
        capabilities.resume = true;
        capabilities.steering = true;
        capabilities
    }

    fn submit(&self, prompt: BackendPrompt) -> BackendFuture<Result<BackendTurn>> {
        let turn = self.start(prompt);
        Box::pin(async move { Ok(turn) })
    }

    fn route(&self, prompt: BackendPrompt) -> BackendFuture<Result<BackendPromptRoute>> {
        let turn = self.start(prompt);
        Box::pin(async move { Ok(BackendPromptRoute::Started(turn)) })
    }

    fn steer(&self, key: BackendTurnKey, prompt: Prompt) -> BackendFuture<Result<()>> {
        self.steer_turn(key, None, prompt)
    }

    fn steer_with_id(
        &self,
        key: BackendTurnKey,
        id: String,
        prompt: Prompt,
    ) -> BackendFuture<Result<()>> {
        self.steer_turn(key, Some(id), prompt)
    }

    fn cancel(&self, key: BackendTurnKey) -> BackendFuture<Result<()>> {
        let Some(request_id) = self.requests.get(&key.0) else {
            return Box::pin(async { Err(NanocodeError::TurnNotCancellable) });
        };
        self.children
            .host
            .cancel(Arc::clone(&self.session_id), Some(request_id))
    }

    fn cancel_active(&self) -> BackendFuture<Result<()>> {
        self.children
            .host
            .cancel(Arc::clone(&self.session_id), None)
    }

    fn set_harness_model(&self, _model: ModelSelection) -> BackendFuture<Result<()>> {
        unsupported("remote_child_model")
    }

    fn set_thinking(&self, _thinking: Thinking) -> BackendFuture<Result<()>> {
        unsupported("remote_child_thinking")
    }

    fn set_fast_mode(&self, _enabled: bool) -> BackendFuture<Result<()>> {
        unsupported("remote_child_fast_mode")
    }

    fn compact(&self) -> BackendFuture<Result<()>> {
        unsupported("compaction")
    }

    fn append_developer_message(
        &self,
        _text: String,
    ) -> BackendFuture<Result<AgentSessionContext>> {
        unsupported("developer_messages")
    }

    fn context(&self) -> BackendFuture<Result<AgentSessionContext>> {
        unsupported("context")
    }

    fn checkpoint(&self) -> BackendFuture<Result<SessionCheckpoint>> {
        let checkpoint = SessionCheckpoint::native(
            self.session_id.to_string(),
            self.child.model.clone(),
            self.child.thinking,
            self.child.lineage.clone(),
            self.session_id.to_string(),
            None,
            true,
            json!({ REMOTE_MARKER: true }),
        );
        Box::pin(async move { Ok(checkpoint) })
    }

    fn spawn(&self, _options: SpawnOptions) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        unsupported("spawn")
    }

    fn fork(&self, _request: ForkRequest) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        unsupported("fork")
    }

    fn flush(&self) -> BackendFuture<Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn shutdown(&self) -> BackendFuture<Result<()>> {
        if self
            .stopped
            .update(|stopped| std::mem::replace(stopped, true))
        {
            return Box::pin(async { Ok(()) });
        }
        self.children.children.remove_if(&self.session_id, |_| true);
        self.children.host.close(Arc::clone(&self.session_id))
    }

    fn is_stopped(&self) -> bool {
        self.stopped.read(|stopped| *stopped)
    }
}
