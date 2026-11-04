use super::*;
#[derive(Clone, Copy, Eq, PartialEq)]

pub(super) struct TurnKey(pub(super) u64);

pub(super) enum Command {
    Prompt {
        key: TurnKey,
        prompt: Prompt,
        execution_operation: Option<ExecutionOperation>,
        accepted: Option<oneshot::Sender<Result<String>>>,
        cancel_on_admission: bool,
        thinking: Option<Thinking>,
        service_tier: Option<ServiceTier>,
        parent: Option<tracing::Span>,
        events: EventSink,
        result: oneshot::Sender<Result<TurnResult>>,
    },
    Steer {
        key: TurnKey,
        prompt: Prompt,
        result: oneshot::Sender<Result<()>>,
    },
    SteerWithId {
        key: TurnKey,
        id: String,
        prompt: Prompt,
        result: oneshot::Sender<Result<()>>,
    },
    WithdrawSteer {
        key: TurnKey,
        id: String,
        result: oneshot::Sender<Result<bool>>,
    },
    RoutePrompt {
        key: TurnKey,
        prompt: Prompt,
        request_id: Option<String>,
        cancel_on_admission: bool,
        parent: Option<tracing::Span>,
        events: EventSink,
        turn_result: oneshot::Sender<Result<TurnResult>>,
        route_result: oneshot::Sender<Result<PromptRouteKind>>,
    },
    Cancel {
        key: TurnKey,
        result: oneshot::Sender<Result<()>>,
    },
    CancelActive {
        result: oneshot::Sender<Result<()>>,
    },
    Preconnect {
        result: oneshot::Sender<Result<()>>,
    },
    Fork {
        origin: crate::Origin,
        point: ForkFrom,
        session_id: SessionId,
        policy: Option<Arc<dyn execution::ExecutionPolicy>>,
        result: oneshot::Sender<Result<(Agent, AgentEvents)>>,
    },

    ChildSnapshot {
        result: oneshot::Sender<Result<ChildState>>,
    },
    Spawn {
        restore: Option<ChildState>,
        options: SpawnOptions,
        host_context: Option<Arc<str>>,
        result: oneshot::Sender<Result<(Agent, AgentEvents)>>,
    },
    SpawnBatch {
        count: usize,
        observer: Option<Arc<SpawnObserver>>,
        host_context: Option<Arc<str>>,
        result: oneshot::Sender<Result<Vec<(Agent, AgentEvents)>>>,
    },
    SetModel {
        model: Model,
        result: oneshot::Sender<Result<()>>,
    },
    SetThinking {
        thinking: Thinking,
        result: oneshot::Sender<Result<()>>,
    },
    SetAutoCompactTokenLimit {
        limit: Option<u64>,
        result: oneshot::Sender<Result<()>>,
    },
    SetCompactionThinking {
        thinking: Option<Thinking>,
        result: oneshot::Sender<Result<()>>,
    },
    SetServiceTier {
        service_tier: ServiceTier,
        result: oneshot::Sender<Result<()>>,
    },
    Compact {
        parent: Option<tracing::Span>,
        result: oneshot::Sender<Result<()>>,
    },
    AppendDeveloperMessage {
        text: String,
        result: DeveloperMessageResult,
    },
    Context {
        result: oneshot::Sender<Result<AgentSessionContext>>,
    },
    SideQuestion {
        question: String,
        result: oneshot::Sender<Result<nanocode_agent_core::SideAnswer>>,
    },
    Shutdown,
}

pub(super) enum DeveloperMessageResult {
    Context(oneshot::Sender<Result<AgentSessionContext>>),
    Notice {
        prefix: Option<String>,
        result: oneshot::Sender<Result<bool>>,
    },
}

impl DeveloperMessageResult {
    pub(super) fn prefix(&self) -> Option<&str> {
        match self {
            Self::Context(_) => None,
            Self::Notice { prefix, .. } => prefix.as_deref(),
        }
    }

    pub(super) fn fail(self, error: NanocodeError) {
        match self {
            Self::Context(result) => drop(result.send(Err(error))),
            Self::Notice { result, .. } => drop(result.send(Err(error))),
        }
    }

    pub(super) fn skip(self) {
        if let Self::Notice { result, .. } = self {
            drop(result.send(Ok(false)));
        }
    }
}

pub(super) enum ForkFrom {
    Latest,

    Live(Arc<CommittedSession>),

    Snapshot(Box<SessionSnapshot>),
}

#[derive(Clone)]
pub(super) enum ExecutionOperation {
    Caller(String),
    Automatic(String),
    Admitted(String),
    AdmittedCaller(String),
    Recovered(String),
}

impl ExecutionOperation {
    pub(super) fn into_id(self) -> String {
        match self {
            Self::Caller(operation_id)
            | Self::Automatic(operation_id)
            | Self::Admitted(operation_id)
            | Self::AdmittedCaller(operation_id)
            | Self::Recovered(operation_id) => operation_id,
        }
    }

    pub(super) fn id(&self) -> &str {
        match self {
            Self::Caller(operation_id)
            | Self::Automatic(operation_id)
            | Self::Admitted(operation_id)
            | Self::AdmittedCaller(operation_id)
            | Self::Recovered(operation_id) => operation_id,
        }
    }

    pub(super) const fn is_recovered(&self) -> bool {
        matches!(self, Self::Recovered(_))
    }

    pub(super) const fn survives_shutdown(&self) -> bool {
        matches!(
            self,
            Self::Caller(_) | Self::AdmittedCaller(_) | Self::Recovered(_)
        )
    }
}

pub(super) enum PromptRouteKind {
    Started { request_id: Option<String> },
    Steered,
}

pub(super) enum QueuedTurn {
    Pending {
        key: TurnKey,
        prompt: Prompt,
        execution_operation: Option<ExecutionOperation>,
        thinking: Thinking,
        service_tier: ServiceTier,
        parent: Option<tracing::Span>,
        events: EventSink,
        result: oneshot::Sender<Result<TurnResult>>,
    },
    Cancelled {
        key: TurnKey,
        prompt: Prompt,
        execution_operation: Option<ExecutionOperation>,
        cancellation_committed: bool,
        thinking: Thinking,
        service_tier: ServiceTier,
        parent: Option<tracing::Span>,
        events: EventSink,
        result: oneshot::Sender<Result<TurnResult>>,
    },
    Stopped {
        prompt: Prompt,
        operation_id: String,
        thinking: Thinking,
        service_tier: ServiceTier,
        events: EventSink,
        result: oneshot::Sender<Result<TurnResult>>,
    },
}
