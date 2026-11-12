#![deny(rustdoc::broken_intra_doc_links)]

mod backend;
mod blob;
mod branch;
mod catalog;
mod child_journal;
mod format;
mod memory;
mod paged;
mod pool;
mod record_cache;
mod session;
mod shared_store;
mod state;

pub use backend::{
    Branches, DurableAgentExt, DurableBackendRegistration, agent_error, agent_store_error,
};
pub use blob::MemoryBlobStore;
pub use branch::{BranchTurn, CheckpointBranch};
pub use catalog::{BranchBoundary, InitialSettings, SessionRecord};
pub use catalog::{
    BranchCursor, BranchExport, BranchExportMode, BranchImport, BranchPage, BranchPoint, CancelTo,
    CancelledPending, IntoSessionStore, LIST_LIMIT, Resumable, SessionPage, SessionQuery,
    SessionReader, SessionStore, SessionSummary, StoredSession, StoredTurn, TranscriptItem,
    TurnStatus,
};
pub use child_journal::{JournalClaim, JournalSeed};
pub use format::{AdmittedTurn, CheckpointFormat, FormatFuture};
pub use memory::MemoryStore;
pub use nanocode_durability_core::{
    After, BlobKey, BlobStore, BlobUrl, Colocation, Committed, Deferred, EncodedPayload, Error,
    EventCursor, EventHit, EventHits, EventOrder, EventPage, EventQuery, EventRange, Examined,
    IndexedState, KeyValue, KeyValueStore, KnownSummary, Ledger, ListedState, ListedSummary,
    LockKey, LocksHeld, Migration, MissingColumn, OwnedCommit, OwnedState, OwnerId, OwnerToken,
    PageCodec, Prepare, Result, RowStore, Settle, SharedBlobStore, Sql, SqlAccess, SqlDialect,
    SqlRow, SqlStatement, SqlStore, StagedStore, StagedWrites, StateCommit, StateCursor, StatePage,
    StateQuery, StateReader, StateStore, StateSummary, StoreError, StoreEvent, StoreFuture,
    StoreHandles, StoreRecord, StoreReplacement, StoreUpdate, StoredEvent, StoredState,
    SummaryUpdate, Versioned, base64_data_url, get_blob, migrate_format, migrate_format_then,
};
pub use paged::{PagedWriter, PayloadReader, Prepared, Sequence, SequenceLog, load_sequence};
pub use pool::pool_state;
pub use record_cache::{CachedStore, RecordCache};
pub use session::{
    Admission, AutomaticAdmission, BeginStep, DurableOwner, DurableSession, InitialAcquisition,
    MetadataExpectation, MetadataWrite, OwnerWrite, ResultTiming, SessionReceipt, SessionWrite,
    StepRecovery, StoredSteer, TitleChange, WeakDurableSession, WriteOutcome,
};
pub use shared_store::SharedStore;
pub use state::{
    DurableState, IdentifiedSteerReceipt, OperationState, OperationStatus, SteerState, StepState,
    StepStatus, Transition,
};
