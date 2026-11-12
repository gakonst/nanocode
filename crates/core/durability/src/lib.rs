mod blob;
mod kv;
mod ledger;
mod lock;
mod payload;
mod row;
mod sql;
mod staged;
mod state;
mod store;

pub use blob::{BlobKey, BlobStore, BlobUrl, SharedBlobStore, base64_data_url, get_blob};
pub use kv::{KeyValue, KeyValueStore, Versioned};
pub use ledger::{After, Deferred, Ledger, Prepare};
pub use lock::{LockKey, LocksHeld};
pub use payload::EncodedPayload;
pub use row::{Examined, PageCodec, RowStore};
pub use sql::{
    Collected, Migration, MissingColumn, Sql, SqlAccess, SqlDialect, SqlRow, SqlStatement,
    SqlStore, Unreachable, migrate_format, migrate_format_then,
};
pub use staged::{StagedStore, StagedWrites};
pub use state::{
    DurableCheckpoint, IdentifiedSteerReceipt, JournalState, OperationState, OperationStatus,
    Operations, RetainedCheckpoint, SteerState, StepState, StepStatus, Transition,
};
pub use store::{
    Colocation, Committed, EventCursor, EventHit, EventHits, EventOrder, EventPage, EventQuery,
    EventRange, IndexedState, KnownSummary, ListedState, ListedSummary, OwnedCommit, OwnedState,
    OwnerId, OwnerToken, Settle, StateCommit, StateCursor, StatePage, StateQuery, StateReader,
    StateStore, StateSummary, StoreError, StoreEvent, StoreFuture, StoreHandles, StoreRecord,
    StoreReplacement, StoreUpdate, StoredEvent, StoredState, SummaryUpdate,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    SteerConflict {
        message_id: String,
    },

    SteerWithdrawn {
        message_id: String,
    },

    Store(StoreError),

    Decode {
        revision: u64,

        source: serde_json::Error,
    },

    InvalidState(String),

    StepChanged {
        operation_id: String,
        step_id: String,
    },

    OperationConflict {
        operation_id: String,
    },

    OperationBlocked {
        operation_id: String,

        pending_id: String,
    },

    StepNotStarted {
        operation_id: String,

        step_id: String,
    },

    OperationTerminal {
        operation_id: String,
    },

    OperationActive {
        operation_id: String,
    },

    ModelOwnerActive,

    ModelOwnerFenced,

    OperationNotClaimed {
        operation_id: String,
    },

    AttemptNotStarted {
        operation_id: String,
    },

    AttemptActive {
        operation_id: String,
    },

    CancellationCheckpointRequired {
        operation_id: String,
    },

    RuntimeUnavailable,

    DriverStopped,

    UnsupportedFamily(String),

    SessionNotFound {
        session_id: String,
    },

    SessionExists {
        session_id: String,
    },

    InvalidPayload(serde_json::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SteerConflict { message_id } => write!(
                formatter,
                "steering message `{message_id}` already has different input"
            ),
            Self::SteerWithdrawn { message_id } => {
                write!(formatter, "steering message `{message_id}` was withdrawn")
            }
            Self::Store(value) => std::fmt::Display::fmt(value, formatter),
            Self::Decode { revision, source } => write!(
                formatter,
                "durability state at revision {revision} is invalid: {source}"
            ),
            Self::InvalidState(value) => write!(formatter, "invalid durability state: {value}"),
            Self::StepChanged {
                operation_id,
                step_id,
            } => write!(
                formatter,
                "invalid durability state: step `{step_id}` in operation `{operation_id}` changed definition"
            ),
            Self::OperationConflict { operation_id } => write!(
                formatter,
                "durable operation `{operation_id}` already has different input"
            ),
            Self::OperationBlocked {
                operation_id,
                pending_id,
            } => write!(
                formatter,
                "durable operation `{operation_id}` is blocked by unfinished operation `{pending_id}`"
            ),
            Self::StepNotStarted {
                operation_id,
                step_id,
            } => write!(
                formatter,
                "durable step `{step_id}` in operation `{operation_id}` was not started"
            ),
            Self::OperationTerminal { operation_id } => write!(
                formatter,
                "durable operation `{operation_id}` is already terminal"
            ),
            Self::OperationActive { operation_id } => write!(
                formatter,
                "durable operation `{operation_id}` is already active"
            ),
            Self::ModelOwnerActive => {
                formatter.write_str("durability state has a live authoritative model owner")
            }
            Self::ModelOwnerFenced => {
                formatter.write_str("durability model owner was fenced by a newer owner")
            }
            Self::OperationNotClaimed { operation_id } => write!(
                formatter,
                "durable operation `{operation_id}` is not claimed by this owner"
            ),
            Self::AttemptNotStarted { operation_id } => write!(
                formatter,
                "durable operation `{operation_id}` has not begun an attempt"
            ),
            Self::AttemptActive { operation_id } => write!(
                formatter,
                "durable operation `{operation_id}` already has an active attempt"
            ),
            Self::CancellationCheckpointRequired { operation_id } => write!(
                formatter,
                "active durable operation `{operation_id}` cancellation requires a checkpoint"
            ),
            Self::RuntimeUnavailable => {
                formatter.write_str("durability requires an active Tokio runtime")
            }
            Self::DriverStopped => formatter.write_str("durability driver stopped"),
            Self::UnsupportedFamily(value) => write!(
                formatter,
                "no checkpoint format is registered for harness family `{value}`"
            ),
            Self::SessionNotFound { session_id } => {
                write!(formatter, "unknown session `{session_id}`")
            }
            Self::SessionExists { session_id } => write!(
                formatter,
                "session `{session_id}` already exists; open it instead"
            ),
            Self::InvalidPayload(value) => {
                write!(formatter, "durability payload is invalid JSON: {value}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(value) => std::error::Error::source(value),
            Self::Decode { source, .. } => Some(source),
            Self::InvalidPayload(value) => Some(value),
            _ => None,
        }
    }
}

impl From<StoreError> for Error {
    fn from(source: StoreError) -> Self {
        Self::Store(source)
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Self::InvalidPayload(source)
    }
}
