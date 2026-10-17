mod payload;
mod state;
mod store;

pub use payload::EncodedPayload;
pub use state::{
    DurableCheckpoint, IdentifiedSteerReceipt, JournalState, OperationState, OperationStatus,
    RetainedCheckpoint, SteerState, StepState, StepStatus, Transition,
};
pub use store::{
    Committed, EventCursor, EventHit, EventHits, EventOrder, EventPage, EventQuery, EventRange,
    IndexedState, OwnedState, OwnerId, OwnerToken, StateCommit, StateCursor, StatePage, StateQuery,
    StateReader, StateStore, StateSummary, StoreError, StoreEvent, StoreFuture, StoreRecord,
    StoreReplacement, StoreUpdate, StoredEvent, StoredState, SummaryUpdate,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("steering message `{message_id}` already has different input")]
    SteerConflict { message_id: String },

    #[error("steering message `{message_id}` was withdrawn")]
    SteerWithdrawn { message_id: String },

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("durability state at revision {revision} is invalid: {source}")]
    Decode {
        revision: u64,

        #[source]
        source: serde_json::Error,
    },

    #[error("invalid durability state: {0}")]
    InvalidState(String),

    #[error("durable operation `{operation_id}` already has different input")]
    OperationConflict { operation_id: String },

    #[error("durable operation `{operation_id}` is blocked by unfinished operation `{pending_id}`")]
    OperationBlocked {
        operation_id: String,

        pending_id: String,
    },

    #[error("durable step `{step_id}` in operation `{operation_id}` was not started")]
    StepNotStarted {
        operation_id: String,

        step_id: String,
    },

    #[error("durable operation `{operation_id}` is already terminal")]
    OperationTerminal { operation_id: String },

    #[error("durable operation `{operation_id}` is already active")]
    OperationActive { operation_id: String },

    #[error("durability state has a live authoritative model owner")]
    ModelOwnerActive,

    #[error("durability model owner was fenced by a newer owner")]
    ModelOwnerFenced,

    #[error("durable operation `{operation_id}` is not claimed by this owner")]
    OperationNotClaimed { operation_id: String },

    #[error("durable operation `{operation_id}` has not begun an attempt")]
    AttemptNotStarted { operation_id: String },

    #[error("durable operation `{operation_id}` already has an active attempt")]
    AttemptActive { operation_id: String },

    #[error("active durable operation `{operation_id}` cancellation requires a checkpoint")]
    CancellationCheckpointRequired { operation_id: String },

    #[error("durability requires an active Tokio runtime")]
    RuntimeUnavailable,

    #[error("durability driver stopped")]
    DriverStopped,

    #[error("no checkpoint format is registered for harness family `{0}`")]
    UnsupportedFamily(String),

    #[error("unknown session `{session_id}`")]
    SessionNotFound { session_id: String },

    #[error("durability payload is invalid JSON: {0}")]
    InvalidPayload(#[from] serde_json::Error),
}
