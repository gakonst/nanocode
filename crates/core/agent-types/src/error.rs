use std::sync::Arc;
use std::{io, path::PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionPolicyDisposition {
    Retry,

    Reopen,

    Fatal,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CompactionRecovery {
    #[default]
    None,

    ReplaceRejectedImages,
}

#[derive(Debug)]
pub enum NanocodeError {
    InvalidRequest(String),

    ResolveWorkspace {
        path: PathBuf,

        source: io::Error,
    },

    WorkspaceNotDirectory {
        path: PathBuf,
    },

    WorkspaceNotUtf8 {
        path: PathBuf,
    },

    WorkspaceChanged {
        current: String,
        requested: String,
    },

    MalformedResponse {
        detail: &'static str,
    },

    InvalidAttemptState {
        detail: &'static str,
    },

    CompactionFailed {
        detail: String,

        requires_session_stop: bool,

        recovery: CompactionRecovery,
    },

    SerializePromptPrefix(serde_json::Error),

    AgentStopped,

    ExecutionPolicyOwnerStopped,

    TurnStopped,

    Shutdown(Arc<Self>),

    TurnNotSteerable,

    SteerQueueFull,

    TurnNotCancellable,

    TurnCancelled,

    ForkBeforeCompletedTurn,

    CheckpointLineageMismatch,

    InvalidCheckpoint(String),

    ExecutionPolicy {
        layer: &'static str,

        disposition: ExecutionPolicyDisposition,

        source: Arc<dyn std::error::Error + Send + Sync>,
    },

    ExecutionPolicyNotConfigured,

    InvalidExecutionPolicy(String),

    BeforeCompactionFailed(String),

    ExecutionPolicyCapabilityUnsupported {
        capability: &'static str,
    },

    ExecutionPolicyBranchUnsupported {
        operation: &'static str,
    },

    ExecutionPayload(serde_json::Error),

    ReplayedCheckpointUnavailable,

    CheckpointFamilyMismatch {
        expected: crate::HarnessFamily,

        found: crate::HarnessFamily,
    },

    UnsupportedCapability {
        capability: &'static str,
    },

    BackendContract {
        detail: &'static str,
    },

    Backend {
        backend: &'static str,

        source: Arc<dyn std::error::Error + Send + Sync>,
    },

    ReplayedExecutionFailed(String),

    TokioRuntimeUnavailable,

    Event(nanocode_types_core::events::EventError),

    Response(BackendError),

    Tools(nanocode_tool_contracts::ToolsBuildError),
}

impl std::fmt::Display for NanocodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(value) => write!(formatter, "invalid task request: {value}"),
            Self::ResolveWorkspace { path, source } => {
                let displayed_path = path.display();
                write!(
                    formatter,
                    "failed to resolve task workspace {displayed_path}: {source}"
                )
            }
            Self::WorkspaceNotDirectory { path } => {
                let displayed_path = path.display();
                write!(
                    formatter,
                    "task workspace is not a directory: {displayed_path}"
                )
            }
            Self::WorkspaceNotUtf8 { path } => {
                let displayed_path = path.display();
                write!(
                    formatter,
                    "task workspace path is not valid UTF-8: {displayed_path}"
                )
            }
            Self::WorkspaceChanged { current, requested } => write!(
                formatter,
                "an active agent session cannot change workspace from {current} to {requested}"
            ),
            Self::MalformedResponse { detail } => {
                write!(formatter, "malformed Responses API event: {detail}")
            }
            Self::InvalidAttemptState { detail } => {
                write!(formatter, "invalid Responses attempt state: {detail}")
            }
            Self::CompactionFailed { detail, .. } => {
                write!(formatter, "compaction failed: {detail}")
            }
            Self::SerializePromptPrefix(value) => write!(
                formatter,
                "failed to fingerprint the immutable prompt prefix: {value}"
            ),
            Self::AgentStopped => {
                formatter.write_str("the agent stopped before accepting the command")
            }
            Self::ExecutionPolicyOwnerStopped => {
                formatter.write_str("the execution-policy-owned agent stopped and must be reopened")
            }
            Self::TurnStopped => formatter.write_str("the agent stopped before the turn completed"),
            Self::Shutdown(value) => std::fmt::Display::fmt(value, formatter),
            Self::TurnNotSteerable => formatter.write_str(
                "the targeted turn is queued, completed, or otherwise not active for steering",
            ),
            Self::SteerQueueFull => formatter.write_str("the active turn's steering queue is full"),
            Self::TurnNotCancellable => {
                formatter.write_str("the targeted turn has already completed or been cancelled")
            }
            Self::TurnCancelled => formatter.write_str("the turn was cancelled"),
            Self::ForkBeforeCompletedTurn => {
                formatter.write_str("the agent has no safe conversation boundary to fork")
            }
            Self::CheckpointLineageMismatch => formatter
                .write_str("the completed turn belongs to a different conversation lineage"),
            Self::InvalidCheckpoint(value) => {
                write!(formatter, "invalid session checkpoint: {value}")
            }
            Self::ExecutionPolicy { layer, source, .. } => match source.downcast_ref::<Self>() {
                Some(agent) => std::fmt::Display::fmt(agent, formatter),
                None => write!(formatter, "{layer} execution policy failed: {source}"),
            },
            Self::ExecutionPolicyNotConfigured => formatter
                .write_str("identified prompt submission requires a configured execution policy"),
            Self::InvalidExecutionPolicy(value) => {
                write!(formatter, "invalid execution policy state: {value}")
            }
            Self::BeforeCompactionFailed(value) => {
                write!(formatter, "pre-compaction preservation failed: {value}")
            }
            Self::ExecutionPolicyCapabilityUnsupported { capability } => write!(
                formatter,
                "execution policy does not implement required capability `{capability}`"
            ),
            Self::ExecutionPolicyBranchUnsupported { operation } => write!(
                formatter,
                "cannot {operation} from an agent with an attached execution policy; build the branch with its own execution policy"
            ),
            Self::ExecutionPayload(value) => {
                write!(formatter, "execution policy payload is invalid: {value}")
            }
            Self::ReplayedCheckpointUnavailable => formatter.write_str(
                "a policy-replayed result cannot be used as an in-process fork checkpoint",
            ),
            Self::CheckpointFamilyMismatch { expected, found } => write!(
                formatter,
                "checkpoint belongs to the {found:?} harness, not {expected:?}"
            ),
            Self::UnsupportedCapability { capability } => write!(
                formatter,
                "the selected agent backend does not support {capability}"
            ),
            Self::BackendContract { detail } => write!(
                formatter,
                "agent backend violated the lifecycle contract: {detail}"
            ),
            Self::Backend { backend, source } => {
                write!(formatter, "{backend} backend failed: {source}")
            }
            Self::ReplayedExecutionFailed(value) => formatter.write_str(value),
            Self::TokioRuntimeUnavailable => {
                formatter.write_str("building an agent requires an active Tokio runtime")
            }
            Self::Event(value) => std::fmt::Display::fmt(value, formatter),
            Self::Response(value) => std::fmt::Display::fmt(value, formatter),
            Self::Tools(value) => write!(
                formatter,
                "failed to build tools for an agent driver: {value}"
            ),
        }
    }
}

impl std::error::Error for NanocodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ResolveWorkspace { source, .. } => Some(source),
            Self::SerializePromptPrefix(value) => Some(value),
            Self::Shutdown(value) => std::error::Error::source(value),
            Self::ExecutionPolicy { source, .. } => match source.downcast_ref::<Self>() {
                Some(agent) => std::error::Error::source(agent),
                None => Some(source),
            },
            Self::ExecutionPayload(value) => Some(value),
            Self::Backend { source, .. } => Some(source),
            Self::Event(value) => std::error::Error::source(value),
            Self::Response(value) => std::error::Error::source(value),
            Self::Tools(value) => Some(value),
            _ => None,
        }
    }
}

impl From<nanocode_types_core::events::EventError> for NanocodeError {
    fn from(source: nanocode_types_core::events::EventError) -> Self {
        Self::Event(source)
    }
}

impl From<nanocode_tool_contracts::ToolsBuildError> for NanocodeError {
    fn from(source: nanocode_tool_contracts::ToolsBuildError) -> Self {
        Self::Tools(source)
    }
}

impl NanocodeError {
    #[doc(hidden)]
    pub fn backend<E>(backend: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Backend {
            backend,
            source: Arc::new(source),
        }
    }

    #[doc(hidden)]
    pub fn execution_policy<E>(layer: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::ExecutionPolicy {
            layer,
            disposition: ExecutionPolicyDisposition::Fatal,
            source: Arc::new(source),
        }
    }

    #[doc(hidden)]
    pub fn execution_policy_with_disposition<E>(
        layer: &'static str,
        disposition: ExecutionPolicyDisposition,
        source: E,
    ) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::ExecutionPolicy {
            layer,
            disposition,
            source: Arc::new(source),
        }
    }

    #[must_use]
    pub fn execution_policy_disposition(&self) -> Option<ExecutionPolicyDisposition> {
        match self {
            Self::ExecutionPolicy { disposition, .. } => Some(*disposition),
            Self::ExecutionPolicyOwnerStopped => Some(ExecutionPolicyDisposition::Reopen),
            Self::Shutdown(source) => source.execution_policy_disposition(),
            _ => None,
        }
    }

    #[must_use]
    pub fn requires_image_repair(&self) -> bool {
        match self {
            Self::CompactionFailed {
                recovery,
                requires_session_stop,
                ..
            } => !requires_session_stop && *recovery == CompactionRecovery::ReplaceRejectedImages,
            Self::Shutdown(source) => source.requires_image_repair(),
            Self::Response(error) => error.requires_image_repair(),
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, NanocodeError>;

pub trait BackendFailure: std::error::Error + Send + Sync + 'static {
    fn requires_image_repair(&self) -> bool {
        false
    }

    fn retryable(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any;
}

#[derive(Debug)]
pub struct BackendError(pub Box<dyn BackendFailure>);
impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for BackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
impl std::ops::Deref for BackendError {
    type Target = dyn BackendFailure;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
